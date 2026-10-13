//! REQ-THERMAL-006/007 bounded child lifecycle tests, no GPU devices.
#![allow(dead_code)]
#[path = "../src/metrics/mod.rs"]
mod metrics;
#[path = "../src/safe_nvml.rs"]
mod safe_nvml;
#[path = "../src/state.rs"]
mod state;
use metrics::nvidia_rm::{RmRequest, RmStatus};
use metrics::thermal_worker::{ThermalManager, WorkerError};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

fn request() -> RmRequest {
    RmRequest {
        pci_bus_id: "0000:23:00.0".into(),
        uuid: "GPU-A".into(),
        device_id: 0x2b85,
        driver_version: "590.48.01".into(),
    }
}
fn fixture(body: &str) -> ThermalManager {
    let prefix = "import sys,json,struct,time\nn=struct.unpack('<I',sys.stdin.buffer.read(4))[0]\nq=json.loads(sys.stdin.buffer.read(n))\n";
    ThermalManager::with_command(
        PathBuf::from("/usr/bin/python3"),
        vec!["-c".into(), format!("{prefix}{body}")],
    )
}

#[test]
fn framed_current_reply_is_accepted() {
    let manager = fixture("r={'request_id':q['request_id'],'readings':[{'pci_bus_id':'0000:23:00.0','uuid':'GPU-A','celsius':72.5,'status':'valid','remaining_validity_us':3000000}]}\nb=json.dumps(r).encode()\nsys.stdout.buffer.write(struct.pack('<I',len(b))+b)\nsys.stdout.buffer.flush()\n");
    let result = manager
        .collect(&[request()], Duration::from_secs(1), Duration::from_secs(3))
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].status, RmStatus::Valid);
    assert_eq!(result[0].celsius, Some(72.5));
}
#[test]
fn hung_and_late_child_are_bounded_and_do_not_reuse_a_reply() {
    let manager = fixture("time.sleep(10)\n");
    let start = Instant::now();
    assert_eq!(
        manager
            .collect(
                &[request()],
                Duration::from_millis(60),
                Duration::from_secs(3)
            )
            .unwrap_err(),
        WorkerError::Timeout
    );
    assert!(start.elapsed() < Duration::from_millis(500));
    let result = manager.collect(
        &[request()],
        Duration::from_millis(60),
        Duration::from_secs(3),
    );
    assert!(matches!(
        result,
        Err(WorkerError::Timeout | WorkerError::Busy)
    ));
}
#[test]
fn malformed_oversized_eof_and_wrong_generation_are_rejected() {
    for body in ["sys.stdout.buffer.write(struct.pack('<I',0xffffffff));sys.stdout.buffer.flush()\n",
        "sys.stdout.buffer.write(struct.pack('<I',3)+b'bad');sys.stdout.buffer.flush()\n", "pass\n",
        "b=json.dumps({'request_id':q['request_id']+1,'readings':[]}).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()\n"] {
        let manager = fixture(body); assert!(manager.collect(&[request()], Duration::from_secs(1), Duration::from_secs(3)).is_err());
    }
}
#[test]
fn concurrent_calls_never_launch_a_second_live_helper() {
    let manager = Arc::new(fixture("time.sleep(10)\n"));
    let a = manager.clone();
    let first = std::thread::spawn(move || {
        a.collect(
            &[request()],
            Duration::from_millis(200),
            Duration::from_secs(3),
        )
    });
    std::thread::sleep(Duration::from_millis(40));
    let start = Instant::now();
    assert_eq!(
        manager
            .collect(
                &[request()],
                Duration::from_millis(200),
                Duration::from_secs(3)
            )
            .unwrap_err(),
        WorkerError::Busy
    );
    assert!(start.elapsed() < Duration::from_millis(80));
    assert_eq!(first.join().unwrap().unwrap_err(), WorkerError::Timeout);
    assert_eq!(manager.max_live_children(), 1);
}

#[test]
fn excessive_or_ambiguous_requests_are_rejected_before_a_child_starts() {
    let manager = ThermalManager::with_command("/does/not/exist".into(), vec![]);
    assert!(manager
        .collect(
            &vec![request(); 65],
            Duration::from_secs(1),
            Duration::from_secs(3)
        )
        .is_err());
    assert!(manager
        .collect(
            &[request(), request()],
            Duration::from_secs(1),
            Duration::from_secs(3)
        )
        .is_err());
    let mut bad = request();
    bad.pci_bus_id = "23:00.0".into();
    assert!(manager
        .collect(&[bad], Duration::from_secs(1), Duration::from_secs(3))
        .is_err());
    assert_eq!(manager.max_live_children(), 0);
}

#[test]
fn reply_then_stalled_cleanup_is_not_accepted_as_a_finished_batch() {
    let manager=fixture("r={'request_id':q['request_id'],'readings':[{'pci_bus_id':'0000:23:00.0','uuid':'GPU-A','celsius':72.5,'status':'valid','remaining_validity_us':3000000}]}\nb=json.dumps(r).encode()\nsys.stdout.buffer.write(struct.pack('<I',len(b))+b)\nsys.stdout.buffer.flush()\ntime.sleep(10)\n");
    assert_eq!(
        manager
            .collect(
                &[request()],
                Duration::from_millis(60),
                Duration::from_secs(3)
            )
            .unwrap_err(),
        WorkerError::Timeout
    );
}

#[test]
fn real_internal_helper_returns_unsupported_without_normal_startup_or_gpu_access() {
    let manager = ThermalManager::with_command(
        PathBuf::from(env!("CARGO_BIN_EXE_dc-exporter")),
        vec!["--thermal-helper".into()],
    );
    let mut unsupported = request();
    unsupported.device_id = 0;
    unsupported.driver_version = "unknown".into();
    unsupported.uuid = "GPU-12345678-1234-1234-1234-123456789abc".into();
    let result = manager
        .collect(
            &[unsupported],
            Duration::from_secs(1),
            Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(result[0].status, RmStatus::Unsupported);
}

#[test]
fn helper_that_never_reads_and_wrong_current_identity_are_bounded_or_rejected() {
    let manager = ThermalManager::with_command(
        "/usr/bin/python3".into(),
        vec!["-c".into(), "import time;time.sleep(10)".into()],
    );
    let start = Instant::now();
    assert_eq!(
        manager
            .collect(
                &[request()],
                Duration::from_millis(60),
                Duration::from_secs(3)
            )
            .unwrap_err(),
        WorkerError::Timeout
    );
    assert!(start.elapsed() < Duration::from_millis(500));
    for body in [
        "r={'request_id':q['request_id'],'readings':[]}\n",
        "r={'request_id':q['request_id'],'readings':[{'pci_bus_id':'0000:24:00.0','uuid':'GPU-A','celsius':72,'status':'valid'}]}\n",
        "r={'request_id':q['request_id'],'readings':[{'pci_bus_id':'0000:23:00.0','uuid':'GPU-OLD','celsius':72,'status':'valid'}]}\n",
    ] {
        let manager=fixture(&format!("{body}b=json.dumps(r).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()\n"));
        assert_eq!(manager.collect(&[request()],Duration::from_secs(1),Duration::from_secs(3)).unwrap_err(),WorkerError::Invalid);
    }
}

#[test]
fn successful_exit_delay_expires_a_current_framed_reading_without_timing_out() {
    let manager=fixture("r={'request_id':q['request_id'],'readings':[{'pci_bus_id':'0000:23:00.0','uuid':'GPU-A','celsius':72.5,'status':'valid','remaining_validity_us':50000}]}\nb=json.dumps(r).encode()\nsys.stdout.buffer.write(struct.pack('<I',len(b))+b)\nsys.stdout.buffer.flush()\ntime.sleep(0.12)\n");
    let result = manager
        .collect(
            &[request()],
            Duration::from_secs(1),
            Duration::from_millis(100),
        )
        .unwrap();
    assert_eq!(result[0].status, RmStatus::Stale);
    assert_eq!(result[0].celsius, None);
    let mut gpus = vec![metrics::GpuMetrics {
        pci_bus_id: "0000:23:00.0".into(),
        uuid: "GPU-A".into(),
        bound_driver: Some("nvidia".into()),
        vram_temp: Some(99),
        ..Default::default()
    }];
    metrics::thermal::merge_readings(&mut gpus, &[result[0].reading.clone()]);
    assert_eq!(gpus[0].vram_temp, None);
    let output = metrics::prometheus::PrometheusFormatter::format(&metrics::AllMetrics {
        gpus,
        ..Default::default()
    });
    assert!(output
        .lines()
        .any(|l| l.starts_with("DCXP_FI_DEV_VRAM_TEMP_VALID{") && l.ends_with(" 0")));
    assert!(output
        .lines()
        .any(|l| l.starts_with("DCXP_FI_DEV_VRAM_TEMP{") && l.ends_with(" 0")));
}

#[test]
fn valid_wire_results_require_finite_bounded_remaining_validity() {
    for extra in [
        "",
        ",'remaining_validity_us':-1",
        ",'remaining_validity_us':1000001",
        ",'remaining_validity_us':None",
    ] {
        let manager=fixture(&format!("r={{'request_id':q['request_id'],'readings':[{{'pci_bus_id':'0000:23:00.0','uuid':'GPU-A','celsius':72.5,'status':'valid'{extra}}}]}}\nb=json.dumps(r).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()\n"));
        assert_eq!(
            manager
                .collect(&[request()], Duration::from_secs(1), Duration::from_secs(1))
                .unwrap_err(),
            WorkerError::Invalid
        );
    }
}

#[test]
fn delayed_second_gpu_sampling_expires_first_result_before_merge() {
    let manager=fixture("time.sleep(0.12)\nr={'request_id':q['request_id'],'readings':[dict(q['requests'][0],celsius=72.5,status='valid',remaining_validity_us=50000),dict(q['requests'][1],celsius=80,status='valid',remaining_validity_us=100000)]}\nb=json.dumps(r).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()\n");
    let mut second = request();
    second.pci_bus_id = "0000:24:00.0".into();
    second.uuid = "GPU-B".into();
    let result = manager
        .collect(
            &[request(), second],
            Duration::from_secs(1),
            Duration::from_millis(100),
        )
        .unwrap();
    assert_eq!(result[0].status, RmStatus::Stale);
    assert_eq!(result[0].celsius, None);
}
