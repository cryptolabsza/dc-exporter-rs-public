#[path = "../src/metrics/nvidia_rm/mod.rs"]
mod nvidia_rm;
use nvidia_rm::{abi::*, rusd::*, RmRequest, RmStatus};
use std::time::Duration;

fn request() -> RmRequest {
    RmRequest {
        pci_bus_id: "00000000:23:00.0".into(),
        uuid: "GPU-01234567-89ab-cdef-0123-456789abcdef".into(),
        device_id: 0x2b85,
        driver_version: "590.48.01".into(),
    }
}

#[test]
fn profile_is_exact_and_platform_gated() {
    assert!(profile_supported(&request(), "linux", "x86_64"));
    for (os, arch) in [
        ("macos", "x86_64"),
        ("linux", "aarch64"),
        ("windows", "x86_64"),
    ] {
        assert!(!profile_supported(&request(), os, arch));
    }
    let mut r = request();
    r.driver_version = "595.71.05".into();
    assert!(!profile_supported(&r, "linux", "x86_64"));
    r.driver_version = "590.48.01".into();
    r.device_id = 0x2684;
    assert!(!profile_supported(&r, "linux", "x86_64"));
}

#[test]
fn input_batch_rejects_duplicate_or_unbounded_device_requests() {
    assert!(validate_requests(&[request()]).is_ok());
    assert_eq!(
        validate_requests(&[request(), request()]),
        Err(RmStatus::Invalid)
    );
    assert_eq!(
        validate_requests(&vec![request(); 33]),
        Err(RmStatus::Invalid)
    );
    let mut bad = request();
    bad.uuid = "GPU-not-a-uuid".into();
    assert_eq!(validate_requests(&[bad]), Err(RmStatus::Invalid));
}

#[test]
fn bdf_keeps_function_and_rejects_malformed_addresses() {
    assert_eq!(
        normalize_bdf("00000000:AB:1f.7"),
        Some("0000:ab:1f.7".into())
    );
    assert_ne!(normalize_bdf("0000:23:00.0"), normalize_bdf("0000:23:00.1"));
    for bad in [
        "23:00.0",
        "10000:23:00.0",
        "0000:23:20.0",
        "0000:23:00.8",
        "../23:00.0",
        "0000:23:00.0/driver",
    ] {
        assert_eq!(normalize_bdf(bad), None);
    }
}

#[test]
fn identity_requires_full_bdf_uuid_minor_and_current_binding() {
    let r = request();
    let mut id = DeviceIdentity {
        pci_bus_id: "0000:23:00.0".into(),
        uuid: r.uuid.clone(),
        vendor_id: 0x10de,
        device_id: 0x2b85,
        minor: 4,
        rm_minor: 4,
        bound_to_nvidia: true,
    };
    assert_eq!(verify_identity(&r, &id), Ok(()));
    id.rm_minor = 0;
    assert_eq!(verify_identity(&r, &id), Err(RmStatus::DeviceLost));
    id.rm_minor = 4;
    id.pci_bus_id = "0000:23:00.1".into();
    assert_eq!(verify_identity(&r, &id), Err(RmStatus::DeviceLost));
    id.pci_bus_id = "0000:23:00.0".into();
    id.uuid = "GPU-wrong".into();
    assert_eq!(verify_identity(&r, &id), Err(RmStatus::DeviceLost));
    id.uuid = r.uuid.clone();
    id.bound_to_nvidia = false;
    assert_eq!(verify_identity(&r, &id), Err(RmStatus::DeviceLost));
    id.bound_to_nvidia = true;
    id.vendor_id = 0x1234;
    assert_eq!(verify_identity(&r, &id), Err(RmStatus::DeviceLost));
}

#[test]
fn signed_24_8_precision_zero_negative_and_overflow() {
    assert_eq!(decode_temperature(55 * 256 + 128), Ok(55.5));
    assert_eq!(decode_temperature(0), Ok(0.0));
    assert_eq!(decode_temperature(-256), Err(RmStatus::Invalid));
    assert_eq!(decode_temperature(i32::MIN), Err(RmStatus::Invalid));
    assert_eq!(decode_temperature(i32::MAX), Err(RmStatus::Invalid));
    assert_eq!(decode_temperature(151 * 256), Err(RmStatus::Invalid));
}

#[test]
fn snapshot_rejects_invalid_and_in_progress_sequences() {
    for marker in [0, u64::MAX, SEQ_START + 1] {
        assert_eq!(
            stable_snapshot(|| (marker, 55 * 256, marker)),
            Err(RmStatus::Invalid)
        );
    }
    assert_eq!(
        stable_snapshot(|| (SEQ_START, 55 * 256, SEQ_START))
            .unwrap()
            .celsius,
        55.0
    );
    assert_eq!(stable_snapshot(|| (17, 55 * 256, 17)).unwrap().marker, 17);
}

#[test]
fn changing_sequence_exhaustion_never_returns_last_plausible_value() {
    let mut calls = 0;
    assert_eq!(
        stable_snapshot(|| {
            calls += 1;
            (10, 55 * 256, 11)
        }),
        Err(RmStatus::Invalid)
    );
    assert_eq!(calls, 10);
}

#[test]
fn sequence_retry_can_recover_a_stable_sample() {
    let mut calls = 0;
    let result = stable_snapshot(|| {
        calls += 1;
        if calls < 3 {
            (10, 55 * 256, 11)
        } else {
            (12, 56 * 256, 12)
        }
    })
    .unwrap();
    assert_eq!(calls, 3);
    assert_eq!(result.celsius, 56.0);
}

#[test]
fn mapped_byte_boundaries_are_checked_before_reading() {
    for length in [
        0,
        MEMORY_VALUE_OFFSET,
        MEMORY_VALUE_OFFSET + 3,
        RUSD_SIZE - 1,
    ] {
        assert_eq!(snapshot_bytes(&vec![0; length]), Err(RmStatus::Invalid));
    }
    let mut page = vec![0; RUSD_SIZE];
    page[MEMORY_TIMESTAMP_OFFSET..MEMORY_TIMESTAMP_OFFSET + 8]
        .copy_from_slice(&10u64.to_le_bytes());
    page[MEMORY_VALUE_OFFSET..MEMORY_VALUE_OFFSET + 4]
        .copy_from_slice(&(55i32 * 256 + 64).to_le_bytes());
    assert_eq!(snapshot_bytes(&page).unwrap().celsius, 55.25);
}

#[test]
fn first_marker_is_stale_until_monotonic_progress_then_expires() {
    let mut freshness = Freshness::default();
    let age = Duration::from_millis(100);
    assert_eq!(freshness.observe(10, Duration::ZERO, age), RmStatus::Stale);
    assert_eq!(
        freshness.observe(10, Duration::from_millis(10), age),
        RmStatus::Stale
    );
    assert_eq!(
        freshness.observe(11, Duration::from_millis(20), age),
        RmStatus::Valid
    );
    assert_eq!(
        freshness.observe(11, Duration::from_millis(120), age),
        RmStatus::Valid
    );
    assert_eq!(
        freshness.observe(11, Duration::from_millis(121), age),
        RmStatus::Stale
    );
    assert_eq!(
        freshness.observe(9, Duration::from_millis(122), age),
        RmStatus::Invalid
    );
}

#[test]
fn completed_sample_expires_while_other_gpus_are_pending() {
    let mut reading = nvidia_rm::RmReading {
        pci_bus_id: request().pci_bus_id,
        uuid: request().uuid,
        celsius: Some(55.0),
        status: RmStatus::Valid,
    };
    expire_completed(
        &mut reading,
        Duration::from_millis(20),
        Duration::from_millis(121),
        Duration::from_millis(100),
    );
    assert_eq!(reading.status, RmStatus::Stale);
    assert_eq!(reading.celsius, None);
}

#[test]
fn panic_in_sample_callback_still_releases_session() {
    let mut mock = Mock::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_session(&mut mock, |_| -> Result<(), RmStatus> {
            panic!("fixture sample failure")
        })
    }));
    assert!(result.is_err());
    assert_eq!(mock.transcript, session_transcript());
}

#[derive(Default)]
struct Mock {
    transcript: Vec<Operation>,
    fail: Option<Operation>,
}

fn session_transcript() -> Vec<Operation> {
    use Operation::*;
    vec![
        OpenControl,
        AllocateClient,
        VerifyIdentity,
        AttachGpu,
        OpenDevice,
        RegisterDevice,
        AllocateDevice,
        AllocateSubdevice,
        AllocateShared,
        OpenMapping,
        MapMemory,
        MapReadOnly,
        PollThermal,
        Unmap,
        UnmapMemory,
        CloseMapping,
        FreeShared,
        FreeSubdevice,
        FreeDevice,
        CloseDevice,
        DetachGpu,
        FreeClient,
        CloseControl,
    ]
}
impl Transport for Mock {
    fn perform(&mut self, op: Operation) -> Result<(), RmStatus> {
        self.transcript.push(op);
        if self.fail == Some(op) {
            Err(RmStatus::PermissionDenied)
        } else {
            Ok(())
        }
    }
}

#[test]
fn thermal_only_session_releases_every_resource_in_reverse_order() {
    let mut mock = Mock::default();
    assert_eq!(with_session(&mut mock, |_| Ok(())), Ok(()));
    assert_eq!(mock.transcript, session_transcript());
    assert!(mock.transcript.contains(&Operation::PollThermal));
    assert_eq!(mock.transcript.last(), Some(&Operation::CloseControl));
}

#[test]
fn every_partial_initialization_failure_releases_completed_acquisitions() {
    let full = session_transcript();
    let split = full.iter().position(|x| *x == Operation::Unmap).unwrap();
    for &fail in &full[..split] {
        let mut mock = Mock {
            fail: Some(fail),
            ..Default::default()
        };
        assert_eq!(
            with_session(&mut mock, |_| Ok(())),
            Err(RmStatus::PermissionDenied)
        );
        assert_eq!(mock.transcript.iter().filter(|x| **x == fail).count(), 1);
        if fail != Operation::OpenControl {
            assert_eq!(mock.transcript.last(), Some(&Operation::CloseControl));
        }
        if mock.transcript.contains(&Operation::AttachGpu) && fail != Operation::AttachGpu {
            assert!(mock.transcript.contains(&Operation::DetachGpu));
        }
        if fail == Operation::AttachGpu {
            assert!(!mock.transcript.contains(&Operation::DetachGpu));
        }
        if fail == Operation::MapReadOnly {
            assert!(!mock.transcript.contains(&Operation::Unmap));
        }
    }
}

#[test]
fn sample_failure_also_releases_session() {
    let mut mock = Mock::default();
    assert_eq!(
        with_session(&mut mock, |_| Err::<(), _>(RmStatus::DeviceLost)),
        Err(RmStatus::DeviceLost)
    );
    assert_eq!(mock.transcript, session_transcript());
}

#[test]
fn cleanup_continues_after_a_driver_release_error() {
    let mut mock = Mock {
        fail: Some(Operation::UnmapMemory),
        ..Default::default()
    };
    assert_eq!(with_session(&mut mock, |_| Ok(())), Ok(()));
    assert_eq!(mock.transcript, session_transcript());
}

#[test]
fn result_wire_status_is_finite_and_no_valid_error_temperature() {
    assert_eq!(
        serde_json::to_string(&RmStatus::PermissionDenied).unwrap(),
        "\"permission_denied\""
    );
    let r = nvidia_rm::RmReading::unavailable(&request(), RmStatus::Stale);
    assert_eq!(r.celsius, None);
    assert_eq!(r.pci_bus_id, request().pci_bus_id);
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn unsupported_platform_never_opens_driver() {
    let result =
        nvidia_rm::collect_batch(&[request()], Duration::from_secs(1), Duration::from_secs(1));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].status, RmStatus::Unsupported);
    assert_eq!(result[0].celsius, None);
}
