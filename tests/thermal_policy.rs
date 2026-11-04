//! REQ-THERMAL-002/003/004/008/010/013 software policy evidence.
#![allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[path = "../src/metrics/mod.rs"]
mod metrics;
#[path = "../src/safe_nvml.rs"]
mod safe_nvml;
#[path = "../src/state.rs"]
mod state;

use metrics::nvidia_rm::{RmReading, RmStatus};
use metrics::thermal::{
    eligible_for_rusd, measured_vram_supported, merge_readings, normalize_bdf, ThermalBackend,
    ThermalMetadata, ThermalPolicy, ThermalStatus,
};
use metrics::{GpuMetrics, MetricSource};
use state::{GpuArchitecture, GpuState};
use std::time::Duration;

fn healthy(bdf: &str, uuid: &str) -> GpuMetrics {
    GpuMetrics {
        pci_bus_id: bdf.into(),
        uuid: uuid.into(),
        device_id: 0x2b85,
        driver_version: "590.48.01".into(),
        bound_driver: Some("nvidia".into()),
        gpu_temp: Some(58),
        power_usage: Some(120.0),
        architecture: GpuArchitecture::Consumer,
        ..Default::default()
    }
}

#[test]
fn thermal_defaults_are_auto_and_only_explicit_cli_values_override_toml() {
    let cfg: config::Config =
        toml::from_str("[thermal]\nvram_backend='rusd'\ntimeout_ms=90\n").unwrap();
    let default = ThermalPolicy::default();
    assert_eq!(ThermalBackend::default(), ThermalBackend::Auto);
    assert_eq!(default.vram_backend, ThermalBackend::Auto);
    for document in ["", "[thermal]\n", "[thermal]\ntimeout_ms=90\n"] {
        let cfg: config::Config = toml::from_str(document).unwrap();
        assert_eq!(cfg.thermal.vram_backend, ThermalBackend::Auto, "{document}");
    }
    assert_eq!(default.timeout_ms, 2000);
    assert_eq!(
        default.sample_age(Duration::from_secs(1)),
        Duration::from_millis(3000)
    );
    assert_eq!(
        cfg.thermal.resolve(None, None, None).unwrap().vram_backend,
        ThermalBackend::Rusd
    );
    let resolved = cfg
        .thermal
        .resolve(Some(ThermalBackend::Off), Some(40), None)
        .unwrap();
    assert_eq!(resolved.vram_backend, ThermalBackend::Off);
    assert_eq!(resolved.timeout_ms, 40);
    assert!(cfg.thermal.resolve(None, Some(0), None).is_err());
    assert!(toml::from_str::<config::Config>("[thermal]\nvram_backend='invented'").is_err());
    for (wire, backend) in [
        ("legacy", ThermalBackend::Legacy),
        ("auto", ThermalBackend::Auto),
        ("nvml", ThermalBackend::Nvml),
        ("rusd", ThermalBackend::Rusd),
        ("off", ThermalBackend::Off),
    ] {
        let cfg: config::Config =
            toml::from_str(&format!("[thermal]\nvram_backend='{wire}'\n")).unwrap();
        assert_eq!(cfg.thermal.resolve(None, None, None).unwrap().vram_backend, backend);
        assert_eq!(
            cfg.thermal.resolve(Some(ThermalBackend::Legacy), None, None).unwrap().vram_backend,
            ThermalBackend::Legacy
        );
    }
}

#[test]
fn full_domain_bdf_is_required_and_canonicalized_without_index_identity() {
    assert_eq!(
        normalize_bdf("00000000:AB:00.0"),
        Some("0000:ab:00.0".into())
    );
    assert_eq!(normalize_bdf("0001:AB:00.0"), Some("0001:ab:00.0".into()));
    for bad in [
        "23:00.0",
        "0000:00:20.0",
        "0000:00:00.8",
        "garbage",
        "10000:00:00.0",
    ] {
        assert_eq!(normalize_bdf(bad), None, "{bad}");
    }
}

#[test]
fn optional_rusd_requires_current_healthy_bound_host_and_exact_candidate() {
    let gpu = healthy("0000:23:00.0", "GPU-A");
    assert!(eligible_for_rusd(&gpu));
    let mut warning = gpu.clone();
    warning.state = GpuState::Warning;
    assert!(eligible_for_rusd(&warning));
    for bad in [
        GpuState::Error,
        GpuState::Unavailable,
        GpuState::HardwareFailure,
        GpuState::VmPassthrough,
    ] {
        let mut g = gpu.clone();
        g.state = bad;
        assert!(!eligible_for_rusd(&g));
    }
    let mut g = gpu.clone();
    g.nvml_essential_query_failed = true;
    assert!(!eligible_for_rusd(&g));
    let mut g = gpu.clone();
    g.source = MetricSource::Vm;
    assert!(!eligible_for_rusd(&g));
    let mut g = gpu.clone();
    g.bound_driver = None;
    assert!(!eligible_for_rusd(&g));
    let mut g = gpu.clone();
    g.bound_driver = Some("vfio-pci".into());
    assert!(!eligible_for_rusd(&g));
    let mut g = gpu.clone();
    g.device_id = 0x2684;
    assert!(!eligible_for_rusd(&g));
    let mut g = gpu.clone();
    g.driver_version = "590.48.02".into();
    assert!(!eligible_for_rusd(&g));
}

// REQ-THERMAL-003/005/009: profile selection cannot bypass host eligibility.
#[test]
fn exact_615_candidate_retains_590_595_and_every_current_host_gate() {
    for driver in ["590.48.01", "595.99.02", "615.71.09"] {
        let mut gpu = healthy("0000:23:00.0", "GPU-A");
        gpu.driver_version = driver.into();
        assert!(eligible_for_rusd(&gpu), "{driver}");
        gpu.state = GpuState::Warning;
        assert!(eligible_for_rusd(&gpu), "{driver}");
        gpu.state = GpuState::Ok;
        let mut rejected = Vec::new();
        for state in [
            GpuState::Error,
            GpuState::Unavailable,
            GpuState::HardwareFailure,
            GpuState::VmPassthrough,
        ] {
            let mut g = gpu.clone();
            g.state = state;
            rejected.push(g);
        }
        let mut g = gpu.clone();
        g.nvml_essential_query_failed = true;
        rejected.push(g);
        let mut g = gpu.clone();
        g.fallen_off_bus = true;
        rejected.push(g);
        let mut g = gpu.clone();
        g.dmesg_hardware_failure = true;
        rejected.push(g);
        let mut g = gpu.clone();
        g.source = MetricSource::Vm;
        rejected.push(g);
        let mut g = gpu.clone();
        g.bound_driver = None;
        rejected.push(g);
        let mut g = gpu.clone();
        g.bound_driver = Some("vfio-pci".into());
        rejected.push(g);
        let mut g = gpu.clone();
        g.device_id = 0x2684;
        rejected.push(g);
        let mut g = gpu.clone();
        g.uuid.clear();
        rejected.push(g);
        let mut g = gpu.clone();
        g.pci_bus_id = "23:00.0".into();
        rejected.push(g);
        for (index, g) in rejected.iter().enumerate() {
            assert!(!eligible_for_rusd(g), "{driver}/gate {index}");
        }
    }
    for driver in [
        "595.99.01",
        "595.99.03",
        "595.99.02-open",
        "595.99.02 ",
        "600.00.00",
        "615.71.08",
        "615.71.10",
        "615.71.09-open",
        "615.71.09 ",
        "615.78.08",
        "999.00.00",
    ] {
        let mut gpu = healthy("0000:23:00.0", "GPU-A");
        gpu.driver_version = driver.into();
        assert!(!eligible_for_rusd(&gpu), "{driver}");
    }
}

#[test]
fn reordered_current_results_join_by_bdf_and_uuid_and_leave_nvml_fields_unchanged() {
    let mut gpus = vec![
        healthy("0000:23:00.0", "GPU-A"),
        healthy("0001:23:00.0", "GPU-B"),
    ];
    let readings = vec![
        RmReading {
            pci_bus_id: "0001:23:00.0".into(),
            uuid: "GPU-B".into(),
            celsius: Some(80.5),
            status: RmStatus::Valid,
        },
        RmReading {
            pci_bus_id: "00000000:23:00.0".into(),
            uuid: "GPU-A".into(),
            celsius: Some(0.0),
            status: RmStatus::Valid,
        },
    ];
    merge_readings(&mut gpus, &readings);
    assert_eq!(gpus[0].vram_temp, Some(0));
    assert_eq!(gpus[1].vram_temp, Some(81));
    assert!(measured_vram_supported(&gpus[0]));
    for g in &gpus {
        assert_eq!(g.gpu_temp, Some(58));
        assert_eq!(g.power_usage, Some(120.0));
        assert_eq!(g.source, MetricSource::Host);
    }
    let output = metrics::prometheus::PrometheusFormatter::format(&metrics::AllMetrics {
        gpus,
        ..Default::default()
    });
    assert!(output.contains("DCXP_FI_DEV_VRAM_TEMP_VALID{"));
    assert!(output.contains("backend=\"rusd\""));
}

#[test]
fn invalid_stale_duplicate_and_wrong_uuid_replies_never_reuse_previous_value() {
    for status in [
        RmStatus::Stale,
        RmStatus::Invalid,
        RmStatus::Timeout,
        RmStatus::DeviceLost,
    ] {
        let mut gpus = vec![healthy("0000:23:00.0", "GPU-A")];
        gpus[0].vram_temp = Some(99);
        merge_readings(
            &mut gpus,
            &[RmReading {
                pci_bus_id: "0000:23:00.0".into(),
                uuid: "GPU-A".into(),
                celsius: Some(70.0),
                status,
            }],
        );
        assert_eq!(gpus[0].vram_temp, None);
        assert!(!measured_vram_supported(&gpus[0]));
    }
    let reply = RmReading {
        pci_bus_id: "0000:23:00.0".into(),
        uuid: "GPU-A".into(),
        celsius: Some(70.0),
        status: RmStatus::Valid,
    };
    let mut gpus = vec![healthy("0000:23:00.0", "GPU-A")];
    merge_readings(&mut gpus, &[reply.clone(), reply]);
    assert_eq!(gpus[0].vram_temp, None);
    let mut gpus = vec![healthy("0000:23:00.0", "GPU-NEW")];
    merge_readings(
        &mut gpus,
        &[RmReading {
            pci_bus_id: "0000:23:00.0".into(),
            uuid: "GPU-OLD".into(),
            celsius: Some(70.0),
            status: RmStatus::Valid,
        }],
    );
    assert_eq!(gpus[0].vram_temp, None);
}

#[test]
fn fabricated_zero_and_approximations_do_not_claim_measured_support() {
    let mut gpu = healthy("0000:23:00.0", "GPU-A");
    gpu.vram_temp = Some(0);
    gpu.architecture = GpuArchitecture::Datacenter;
    assert!(!measured_vram_supported(&gpu));
    gpu.vram_thermal = Some(ThermalMetadata {
        backend: ThermalBackend::Rusd,
        status: ThermalStatus::Valid,
    });
    assert!(measured_vram_supported(&gpu));
    gpu.vram_temp_approximated = true;
    assert!(!measured_vram_supported(&gpu));
}

#[test]
fn legacy_and_off_never_launch_helper_and_off_preserves_core_and_hotspot() {
    use metrics::thermal_worker::ThermalManager;
    let manager = ThermalManager::with_command("/does/not/exist".into(), vec![]);
    let mut gpu = healthy("0000:23:00.0", "GPU-A");
    gpu.vram_temp = Some(72);
    gpu.hotspot_temp = Some(80);
    let mut gpus = vec![gpu];
    ThermalPolicy {
        vram_backend: ThermalBackend::Legacy,
        ..Default::default()
    }
    .enrich(&mut gpus, &manager, Duration::from_secs(10));
    assert_eq!(gpus[0].vram_temp, Some(72));
    assert_eq!(manager.max_live_children(), 0);
    let off = ThermalPolicy {
        vram_backend: ThermalBackend::Off,
        ..Default::default()
    };
    off.enrich(&mut gpus, &manager, Duration::from_secs(10));
    assert_eq!(gpus[0].vram_temp, None);
    assert_eq!(gpus[0].gpu_temp, Some(58));
    assert_eq!(gpus[0].hotspot_temp, Some(80));
    assert_eq!(manager.max_live_children(), 0);
}

#[test]
fn forced_rusd_does_not_keep_a_legacy_reading_for_unknown_profile() {
    use metrics::thermal_worker::ThermalManager;
    let manager = ThermalManager::with_command("/does/not/exist".into(), vec![]);
    let mut gpu = healthy("0000:23:00.0", "GPU-A");
    gpu.device_id = 0x2684;
    gpu.vram_temp = Some(72);
    let mut gpus = vec![gpu];
    ThermalPolicy {
        vram_backend: ThermalBackend::Rusd,
        ..Default::default()
    }
    .enrich(&mut gpus, &manager, Duration::from_secs(10));
    assert_eq!(gpus[0].vram_temp, None);
    assert_eq!(manager.max_live_children(), 0);
    assert_eq!(
        gpus[0].vram_thermal.as_ref().unwrap().status,
        ThermalStatus::Unsupported
    );
}

#[test]
fn exact_nvml_memory_field_checks_value_type_identity_and_documented_timestamp() {
    use metrics::thermal::decode_nvml_sample;
    use nvml_wrapper::{
        enums::device::SampleValue, struct_wrappers::device::FieldValueSample,
        structs::device::FieldId,
    };
    let sample = |field, timestamp, value| FieldValueSample {
        field: FieldId(field),
        timestamp,
        latency: 0,
        value: Ok(value),
    };
    let age = Duration::from_secs(3);
    assert_eq!(
        decode_nvml_sample(&sample(82, 9_000_000, SampleValue::U32(0)), 10_000_000, age),
        Ok(0.0)
    );
    assert_eq!(
        decode_nvml_sample(
            &sample(82, 9_000_000, SampleValue::F64(72.5)),
            10_000_000,
            age
        ),
        Ok(72.5)
    );
    assert_eq!(
        decode_nvml_sample(
            &sample(81, 9_000_000, SampleValue::U32(72)),
            10_000_000,
            age
        ),
        Err(ThermalStatus::Invalid)
    );
    assert_eq!(
        decode_nvml_sample(
            &sample(82, 1_000_000, SampleValue::U32(72)),
            10_000_000,
            age
        ),
        Err(ThermalStatus::Stale)
    );
    assert_eq!(
        decode_nvml_sample(
            &sample(82, 11_000_000, SampleValue::U32(72)),
            10_000_000,
            age
        ),
        Err(ThermalStatus::Invalid)
    );
    assert_eq!(
        decode_nvml_sample(&sample(82, 0, SampleValue::U32(72)), 10_000_000, age),
        Err(ThermalStatus::Invalid)
    );
    assert_eq!(
        decode_nvml_sample(
            &sample(82, 9_000_000, SampleValue::F64(f64::NAN)),
            10_000_000,
            age
        ),
        Err(ThermalStatus::Invalid)
    );
}

#[test]
fn public_cli_exposes_thermal_selection_and_rejects_invalid_deadline_before_collection() {
    let binary = env!("CARGO_BIN_EXE_dc-exporter");
    let help = std::process::Command::new(binary)
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("--vram-backend"));
    assert!(text.contains("--thermal-timeout-ms"));
    assert!(!text.contains("--thermal-helper"));
    let result = std::process::Command::new(binary)
        .args([
            "--one-shot",
            "--no-register-access",
            "--thermal-timeout-ms",
            "0",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("thermal timeout_ms"));
}

#[test]
fn auto_falls_back_only_on_unsupported_and_forced_nvml_never_uses_legacy() {
    use metrics::thermal_worker::ThermalManager;
    for (backend, status, expected) in [
        (ThermalBackend::Auto, "unsupported", Some(72)),
        (ThermalBackend::Auto, "invalid", None),
        (ThermalBackend::Auto, "stale", None),
        (ThermalBackend::Auto, "permission_denied", None),
        (ThermalBackend::Auto, "timeout", None),
        (ThermalBackend::Auto, "device_lost", None),
        (ThermalBackend::Nvml, "unsupported", None),
    ] {
        let code=format!("import sys,json,struct\nn=struct.unpack('<I',sys.stdin.buffer.read(4))[0];q=json.loads(sys.stdin.buffer.read(n))\nr={{'request_id':q['request_id'],'readings':[{{'backend':'nvml','pci_bus_id':'0000:23:00.0','uuid':'GPU-A','celsius':None,'status':'{status}'}}]}}\nb=json.dumps(r).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()\n");
        let manager =
            ThermalManager::with_command("/usr/bin/python3".into(), vec!["-c".into(), code]);
        let mut gpu = healthy("0000:23:00.0", "GPU-A");
        gpu.vram_temp = Some(72);
        gpu.vram_thermal = Some(ThermalMetadata {
            backend: ThermalBackend::Legacy,
            status: ThermalStatus::Valid,
        });
        let mut gpus = vec![gpu];
        ThermalPolicy {
            vram_backend: backend,
            ..Default::default()
        }
        .enrich(&mut gpus, &manager, Duration::from_secs(10));
        assert_eq!(gpus[0].vram_temp, expected, "{backend:?}/{status}");
    }
}

fn reply_manager(readings: serde_json::Value) -> metrics::thermal_worker::ThermalManager {
    let encoded = serde_json::to_string(&readings.to_string()).unwrap();
    let code = format!(
        "import sys,json,struct\nn=struct.unpack('<I',sys.stdin.buffer.read(4))[0];q=json.loads(sys.stdin.buffer.read(n))\nassert q['backend']=='auto'\nr={{'request_id':q['request_id'],'readings':json.loads({encoded})}}\nb=json.dumps(r).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()\n"
    );
    metrics::thermal_worker::ThermalManager::with_command(
        "/usr/bin/python3".into(), vec!["-c".into(), code],
    )
}

// TRACE-THERMAL-003/008/012: selection is independent for every current GPU.
#[test]
fn auto_mixed_official_memory_and_unsupported_legacy_preserve_hotspots() {
    let cards = [
        ("NVIDIA A100-SXM4-80GB", 0x20b0, Some(0), Some(49)),
        ("NVIDIA H100 PCIe", 0x2331, Some(0), Some(55)),
        ("NVIDIA RTX A6000", 0x2230, Some(70), None),
        ("NVIDIA GeForce RTX 4090", 0x2684, Some(72), None),
        ("NVIDIA RTX 6000 Ada Generation", 0x26b1, Some(74), None),
        ("NVIDIA L40S", 0x26b9, Some(76), None),
    ];
    let mut gpus = Vec::new();
    let mut replies = Vec::new();
    for (index, (name, device_id, legacy, official)) in cards.iter().enumerate() {
        let mut gpu = healthy(&format!("0000:{:02x}:00.0", 0x20 + index), &format!("GPU-{index}"));
        gpu.name = (*name).into();
        gpu.device_id = *device_id;
        gpu.architecture = GpuArchitecture::from_name(name);
        gpu.vram_temp = *legacy;
        gpu.hotspot_temp = Some(83);
        if official.is_none() {
            gpu.vram_thermal = Some(ThermalMetadata {
                backend: ThermalBackend::Legacy, status: ThermalStatus::Valid,
            });
        }
        replies.push(serde_json::json!({
            "backend": "nvml", "pci_bus_id": gpu.pci_bus_id, "uuid": gpu.uuid,
            "celsius": official, "status": if official.is_some() { "valid" } else { "unsupported" },
            "remaining_validity_us": 30_000_000,
        }));
        gpus.push(gpu);
    }
    // A helper may return a different enumeration order; only BDF + UUID join it.
    replies.reverse();
    let manager = reply_manager(serde_json::Value::Array(replies));
    ThermalPolicy { vram_backend: ThermalBackend::Auto, ..Default::default() }
        .enrich(&mut gpus, &manager, Duration::from_secs(10));
    assert_eq!(manager.max_live_children(), 1);
    for (gpu, (_, _, legacy, official)) in gpus.iter().zip(cards) {
        assert_eq!(gpu.vram_temp, official.or(legacy), "{}", gpu.name);
        assert_eq!(gpu.hotspot_temp, Some(83), "{}", gpu.name);
        assert_eq!(gpu.gpu_temp, Some(58));
        assert_eq!(gpu.power_usage, Some(120.0));
        assert!(measured_vram_supported(gpu), "{}", gpu.name);
        assert_eq!(gpu.vram_thermal, Some(ThermalMetadata {
            backend: if official.is_some() { ThermalBackend::Nvml } else { ThermalBackend::Legacy },
            status: ThermalStatus::Valid,
        }));
    }
}

#[test]
fn auto_unsupported_cannot_promote_datacenter_zero_or_approximated_legacy() {
    for approximated in [false, true] {
        let mut gpu = healthy("0000:23:00.0", "GPU-A");
        gpu.architecture = GpuArchitecture::Datacenter;
        gpu.vram_temp = Some(if approximated { 72 } else { 0 });
        gpu.vram_temp_approximated = approximated;
        let manager = reply_manager(serde_json::json!([{
            "backend":"nvml", "pci_bus_id":gpu.pci_bus_id, "uuid":gpu.uuid,
            "celsius":null, "status":"unsupported",
        }]));
        let mut gpus = vec![gpu];
        ThermalPolicy { vram_backend: ThermalBackend::Auto, ..Default::default() }
            .enrich(&mut gpus, &manager, Duration::from_secs(10));
        assert_eq!(gpus[0].vram_temp, None);
        assert!(!measured_vram_supported(&gpus[0]));
        assert_eq!(gpus[0].vram_thermal.as_ref().unwrap().status, ThermalStatus::Unsupported);
    }
}

#[test]
fn auto_never_launches_helper_for_unhealthy_or_ambiguous_host_identity() {
    for case in 0..11 {
        let mut gpu = healthy("0000:23:00.0", "GPU-A");
        match case {
            0 => gpu.state = GpuState::Error,
            1 => gpu.state = GpuState::Unavailable,
            2 => gpu.state = GpuState::HardwareFailure,
            3 => gpu.state = GpuState::VmPassthrough,
            4 => gpu.nvml_essential_query_failed = true,
            5 => gpu.fallen_off_bus = true,
            6 => gpu.dmesg_hardware_failure = true,
            7 => gpu.bound_driver = Some("vfio-pci".into()),
            8 => gpu.uuid.clear(),
            9 => gpu.pci_bus_id = "23:00.0".into(),
            _ => (),
        }
        let mut gpus = vec![gpu.clone()];
        if case == 10 { gpus.push(gpu); }
        let manager = metrics::thermal_worker::ThermalManager::with_command("/does/not/exist".into(), vec![]);
        ThermalPolicy { vram_backend: ThermalBackend::Auto, ..Default::default() }
            .enrich(&mut gpus, &manager, Duration::from_secs(10));
        assert_eq!(manager.max_live_children(), 0, "case {case}");
        for gpu in gpus {
            assert_eq!(gpu.vram_temp, None, "case {case}");
            assert_eq!(gpu.vram_thermal.as_ref().unwrap().status, ThermalStatus::Unsupported);
        }
    }
}

#[test]
fn malformed_explicit_thermal_config_fails_before_tui_or_collection_startup() {
    let path = std::env::temp_dir().join(format!(
        "dc-exporter-thermal-invalid-{}.toml",
        std::process::id()
    ));
    std::fs::write(
        &path,
        "[thermal]\nvram_backend='misspelled_forced_backend'\n",
    )
    .unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_dc-exporter"))
        .args(["--tui", "--no-register-access", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("failed to load configuration"));
}

#[test]
fn nvml_per_field_errors_precede_unset_or_stale_timestamps_and_control_fallback() {
    use metrics::thermal::decode_nvml_sample;
    use metrics::thermal_worker::ThermalManager;
    use nvml_wrapper::{
        error::NvmlError, struct_wrappers::device::FieldValueSample, structs::device::FieldId,
    };
    for timestamp in [0, 1_000_000] {
        for (error, expected, wire, legacy) in [
            (
                NvmlError::NotSupported,
                ThermalStatus::Unsupported,
                "unsupported",
                Some(72),
            ),
            (
                NvmlError::FunctionNotFound,
                ThermalStatus::Unsupported,
                "unsupported",
                Some(72),
            ),
            (
                NvmlError::NoPermission,
                ThermalStatus::PermissionDenied,
                "permission_denied",
                None,
            ),
            (
                NvmlError::GpuLost,
                ThermalStatus::DeviceLost,
                "device_lost",
                None,
            ),
        ] {
            let sample = FieldValueSample {
                field: FieldId(82),
                timestamp,
                latency: 0,
                value: Err(error),
            };
            assert_eq!(
                decode_nvml_sample(&sample, 10_000_000, Duration::from_secs(3)),
                Err(expected)
            );
            let code=format!("import sys,json,struct\nn=struct.unpack('<I',sys.stdin.buffer.read(4))[0];q=json.loads(sys.stdin.buffer.read(n))\nr={{'request_id':q['request_id'],'readings':[{{'backend':'nvml','pci_bus_id':'0000:23:00.0','uuid':'GPU-A','celsius':None,'status':'{wire}'}}]}}\nb=json.dumps(r).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()\n");
            let manager =
                ThermalManager::with_command("/usr/bin/python3".into(), vec!["-c".into(), code]);
            let mut gpu = healthy("0000:23:00.0", "GPU-A");
            gpu.vram_temp = Some(72);
            let mut gpus = vec![gpu];
            ThermalPolicy {
                vram_backend: ThermalBackend::Auto,
                ..Default::default()
            }
            .enrich(&mut gpus, &manager, Duration::from_secs(10));
            assert_eq!(gpus[0].vram_temp, legacy);
        }
    }
}

#[test]
fn host_modes_preserve_measured_guest_and_off_explicitly_suppresses_guest_vram() {
    use metrics::thermal_worker::ThermalManager;
    for backend in [
        ThermalBackend::Auto,
        ThermalBackend::Nvml,
        ThermalBackend::Rusd,
    ] {
        let manager = ThermalManager::with_command("/does/not/exist".into(), vec![]);
        let mut guest = healthy("0000:23:00.0", "GPU-GUEST");
        guest.source = MetricSource::Vm;
        guest.state = GpuState::VmPassthrough;
        guest.vram_temp = Some(72);
        guest.gpu_temp = Some(58);
        guest.hotspot_temp = Some(80);
        let mut gpus = vec![guest];
        ThermalPolicy {
            vram_backend: backend,
            ..Default::default()
        }
        .enrich(&mut gpus, &manager, Duration::from_secs(10));
        assert_eq!(gpus[0].vram_temp, Some(72));
        assert_eq!(gpus[0].vram_thermal, None);
        assert_eq!(gpus[0].source, MetricSource::Vm);
        assert_eq!(gpus[0].state, GpuState::VmPassthrough);
        assert!(measured_vram_supported(&gpus[0]));
        assert_eq!(manager.max_live_children(), 0);
        let output = metrics::prometheus::PrometheusFormatter::format(&metrics::AllMetrics {
            nvml_gpus: 0,
            total_pci_gpus: 1,
            gpus_available: true,
            gpus: gpus.clone(),
            ..Default::default()
        });
        assert!(output
            .lines()
            .any(|l| l.starts_with("DCXP_FI_DEV_VRAM_TEMP_VALID{")
                && l.contains("source=\"vm\"")
                && l.ends_with(" 1")));
        assert!(output
            .lines()
            .any(|l| l.starts_with("DCXP_FI_DEV_VRAM_TEMP{") && l.ends_with(" 72")));
        assert!(output
            .lines()
            .any(|l| l.starts_with("DCXP_GPU_SUPPORTED{") && l.ends_with(" 1")));
        ThermalPolicy {
            vram_backend: ThermalBackend::Off,
            ..Default::default()
        }
        .enrich(&mut gpus, &manager, Duration::from_secs(10));
        assert_eq!(gpus[0].vram_temp, None);
        assert_eq!(gpus[0].gpu_temp, Some(58));
        assert_eq!(gpus[0].hotspot_temp, Some(80));
    }
}
