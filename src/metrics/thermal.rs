//! TRACE-THERMAL-002/003/004/010/013: optional thermal policy and provenance.
use super::nvidia_rm::{RmReading, RmRequest, RmStatus};
use super::thermal_worker::{ThermalManager, ThermalReading, WorkerError};
use super::{GpuMetrics, MetricSource};
use crate::state::{GpuArchitecture, GpuState};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ThermalBackend {
    #[default]
    Legacy,
    Auto,
    Nvml,
    Rusd,
    Off,
}
impl ThermalBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Auto => "auto",
            Self::Nvml => "nvml",
            Self::Rusd => "rusd",
            Self::Off => "off",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThermalStatus {
    Valid,
    Unsupported,
    PermissionDenied,
    Invalid,
    Stale,
    Timeout,
    DeviceLost,
    Disabled,
}
impl ThermalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Unsupported => "unsupported",
            Self::PermissionDenied => "permission_denied",
            Self::Invalid => "invalid",
            Self::Stale => "stale",
            Self::Timeout => "timeout",
            Self::DeviceLost => "device_lost",
            Self::Disabled => "disabled",
        }
    }
}
impl From<RmStatus> for ThermalStatus {
    fn from(s: RmStatus) -> Self {
        match s {
            RmStatus::Valid => Self::Valid,
            RmStatus::Unsupported => Self::Unsupported,
            RmStatus::PermissionDenied => Self::PermissionDenied,
            RmStatus::Invalid => Self::Invalid,
            RmStatus::Stale => Self::Stale,
            RmStatus::Timeout => Self::Timeout,
            RmStatus::DeviceLost => Self::DeviceLost,
            RmStatus::Disabled => Self::Disabled,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThermalMetadata {
    pub backend: ThermalBackend,
    pub status: ThermalStatus,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThermalPolicy {
    pub vram_backend: ThermalBackend,
    pub timeout_ms: u64,
    pub max_sample_age_ms: Option<u64>,
}
impl Default for ThermalPolicy {
    fn default() -> Self {
        Self {
            vram_backend: ThermalBackend::Legacy,
            timeout_ms: 2000,
            max_sample_age_ms: None,
        }
    }
}
impl ThermalPolicy {
    pub fn resolve(
        &self,
        backend: Option<ThermalBackend>,
        timeout: Option<u64>,
        age: Option<u64>,
    ) -> anyhow::Result<Self> {
        let policy = Self {
            vram_backend: backend.unwrap_or(self.vram_backend),
            timeout_ms: timeout.unwrap_or(self.timeout_ms),
            max_sample_age_ms: age.or(self.max_sample_age_ms),
        };
        anyhow::ensure!(
            (1..=60_000).contains(&policy.timeout_ms),
            "thermal timeout_ms must be 1..=60000"
        );
        anyhow::ensure!(
            policy.max_sample_age_ms != Some(0),
            "thermal max_sample_age_ms must be positive"
        );
        Ok(policy)
    }
    pub fn sample_age(&self, interval: Duration) -> Duration {
        self.max_sample_age_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| {
                interval
                    .saturating_mul(3)
                    .max(Duration::from_millis(self.timeout_ms))
            })
    }
    pub fn enrich(&self, gpus: &mut [GpuMetrics], manager: &ThermalManager, interval: Duration) {
        if self.vram_backend == ThermalBackend::Legacy {
            return;
        }
        let mut requests = Vec::new();
        let counts = identity_counts(gpus);
        for gpu in gpus.iter_mut() {
            if self.vram_backend == ThermalBackend::Off {
                gpu.vram_temp = None;
                gpu.vram_temp_approximated = false;
                gpu.vram_thermal = Some(ThermalMetadata {
                    backend: ThermalBackend::Off,
                    status: ThermalStatus::Disabled,
                });
                continue;
            }
            // Optional NVML/RUSD modes enrich hosts; guest-agent telemetry is independent.
            if gpu.source == MetricSource::Vm {
                continue;
            }
            let healthy = eligible_host(gpu);
            let unique = normalize_bdf(&gpu.pci_bus_id)
                .map(|b| counts.get(&b) == Some(&1))
                .unwrap_or(false);
            if self.vram_backend != ThermalBackend::Auto {
                gpu.vram_temp = None;
                gpu.vram_temp_approximated = false;
            }
            if !healthy
                || !unique
                || (self.vram_backend == ThermalBackend::Rusd && !eligible_for_rusd(gpu))
            {
                gpu.vram_thermal = Some(ThermalMetadata {
                    backend: self.vram_backend,
                    status: ThermalStatus::Unsupported,
                });
                continue;
            }
            requests.push(RmRequest {
                pci_bus_id: normalize_bdf(&gpu.pci_bus_id).unwrap(),
                uuid: gpu.uuid.clone(),
                device_id: gpu.device_id,
                driver_version: gpu.driver_version.clone(),
            });
        }
        if requests.is_empty() {
            return;
        }
        match manager.collect_with_backend(
            &requests,
            Duration::from_millis(self.timeout_ms),
            self.sample_age(interval),
            self.vram_backend,
        ) {
            Ok(readings) => {
                for gpu in gpus.iter_mut().filter(|g| {
                    requests
                        .iter()
                        .any(|r| same_identity(&r.pci_bus_id, &r.uuid, g))
                }) {
                    merge_backend_readings(
                        std::slice::from_mut(gpu),
                        &readings,
                        self.vram_backend == ThermalBackend::Auto,
                    );
                }
            }
            Err(error) => {
                let status = match error {
                    WorkerError::Timeout | WorkerError::Busy => ThermalStatus::Timeout,
                    WorkerError::Unsupported => ThermalStatus::Unsupported,
                    _ => ThermalStatus::Invalid,
                };
                for gpu in gpus.iter_mut().filter(|g| {
                    requests
                        .iter()
                        .any(|r| same_identity(&r.pci_bus_id, &r.uuid, g))
                }) {
                    gpu.vram_temp = None;
                    gpu.vram_temp_approximated = false;
                    gpu.vram_thermal = Some(ThermalMetadata {
                        backend: self.vram_backend,
                        status,
                    });
                }
            }
        }
    }
}

pub fn normalize_bdf(value: &str) -> Option<String> {
    let parts: Vec<_> = value.split([':', '.']).collect();
    if parts.len() != 4
        || !matches!(parts[0].len(), 4 | 8)
        || parts[1].len() != 2
        || parts[2].len() != 2
        || parts[3].len() != 1
        || !parts
            .iter()
            .all(|p| p.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return None;
    }
    let domain = u32::from_str_radix(parts[0], 16).ok()?;
    let bus = u8::from_str_radix(parts[1], 16).ok()?;
    let device = u8::from_str_radix(parts[2], 16).ok()?;
    let function = u8::from_str_radix(parts[3], 16).ok()?;
    if domain > 0xffff || device > 31 || function > 7 {
        return None;
    }
    Some(format!("{domain:04x}:{bus:02x}:{device:02x}.{function}"))
}
pub fn eligible_host(gpu: &GpuMetrics) -> bool {
    gpu.source == MetricSource::Host
        && matches!(gpu.state, GpuState::Ok | GpuState::Warning)
        && !gpu.nvml_essential_query_failed
        && !gpu.fallen_off_bus
        && !gpu.dmesg_hardware_failure
        && gpu.bound_driver.as_deref() == Some("nvidia")
        && !gpu.uuid.is_empty()
        && normalize_bdf(&gpu.pci_bus_id).is_some()
}
pub fn eligible_for_rusd(gpu: &GpuMetrics) -> bool {
    eligible_host(gpu)
        && gpu.device_id == 0x2b85
        && super::nvidia_rm::abi::profile_for_driver(&gpu.driver_version).is_some()
}
fn same_identity(bdf: &str, uuid: &str, gpu: &GpuMetrics) -> bool {
    normalize_bdf(bdf).is_some()
        && normalize_bdf(bdf) == normalize_bdf(&gpu.pci_bus_id)
        && uuid == gpu.uuid
}
fn identity_counts(gpus: &[GpuMetrics]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for g in gpus {
        if let Some(b) = normalize_bdf(&g.pci_bus_id) {
            *counts.entry(b).or_insert(0) += 1;
        }
    }
    counts
}
pub fn merge_readings(gpus: &mut [GpuMetrics], readings: &[RmReading]) {
    let readings: Vec<_> = readings
        .iter()
        .cloned()
        .map(|reading| ThermalReading {
            backend: ThermalBackend::Rusd,
            reading,
            remaining_validity_us: None,
            sample_timestamp_us: None,
            age_started_at: None,
        })
        .collect();
    merge_backend_readings(gpus, &readings, false);
}
fn merge_backend_readings(
    gpus: &mut [GpuMetrics],
    readings: &[ThermalReading],
    allow_legacy_fallback: bool,
) {
    let counts = identity_counts(gpus);
    for gpu in gpus {
        if !eligible_host(gpu) {
            continue;
        }
        let matching: Vec<_> = readings
            .iter()
            .filter(|r| same_identity(&r.reading.pci_bus_id, &r.reading.uuid, gpu))
            .collect();
        let unique = normalize_bdf(&gpu.pci_bus_id)
            .map(|b| counts.get(&b) == Some(&1))
            .unwrap_or(false);
        if !unique || matching.len() != 1 {
            gpu.vram_temp = None;
            gpu.vram_temp_approximated = false;
            gpu.vram_thermal = Some(ThermalMetadata {
                backend: ThermalBackend::Rusd,
                status: ThermalStatus::Invalid,
            });
            continue;
        }
        let result = matching[0];
        if result.reading.status == RmStatus::Unsupported
            && allow_legacy_fallback
            && measured_vram_supported(gpu)
        {
            continue;
        }
        let mut status = ThermalStatus::from(result.reading.status);
        let temp = result
            .reading
            .celsius
            .filter(|t| t.is_finite() && (0.0..=150.0).contains(t));
        if status == ThermalStatus::Valid && temp.is_none() {
            status = ThermalStatus::Invalid;
        }
        gpu.vram_temp = if status == ThermalStatus::Valid {
            temp.map(|t| t.round() as u32)
        } else {
            None
        };
        gpu.vram_temp_approximated = false;
        gpu.vram_thermal = Some(ThermalMetadata {
            backend: result.backend,
            status,
        });
    }
}
pub fn measured_vram_supported(gpu: &GpuMetrics) -> bool {
    let healthy_state = matches!(gpu.state, GpuState::Ok | GpuState::Warning)
        || (gpu.source == MetricSource::Vm && gpu.state == GpuState::VmPassthrough);
    if !healthy_state
        || gpu.nvml_essential_query_failed
        || gpu.vram_temp.is_none()
        || gpu.vram_temp_approximated
    {
        return false;
    }
    if let Some(metadata) = &gpu.vram_thermal {
        return metadata.status == ThermalStatus::Valid;
    }
    // Preserve guest and legacy readings, but reject the datacenter NVML placeholder.
    !(gpu.source == MetricSource::Host
        && gpu.architecture == GpuArchitecture::Datacenter
        && gpu.vram_temp == Some(0))
}
pub fn decode_nvml_sample(
    sample: &nvml_wrapper::struct_wrappers::device::FieldValueSample,
    now_us: i64,
    max_age: Duration,
) -> Result<f64, ThermalStatus> {
    use nvml_wrapper::enums::device::SampleValue;
    if sample.field.0 != nvml_wrapper::sys_exports::field_id::NVML_FI_DEV_MEMORY_TEMP {
        return Err(ThermalStatus::Invalid);
    }
    // Per-field errors may legitimately have an unset timestamp; preserve their status.
    let value = match &sample.value {
        Ok(SampleValue::F64(t)) => *t,
        Ok(SampleValue::U32(t)) => *t as f64,
        Ok(SampleValue::U64(t)) => *t as f64,
        Ok(SampleValue::I64(t)) => *t as f64,
        Err(error) => return Err(nvml_status(error)),
    };
    if sample.timestamp <= 0 || sample.timestamp > now_us {
        return Err(ThermalStatus::Invalid);
    }
    if (now_us - sample.timestamp) as u128 > max_age.as_micros() {
        return Err(ThermalStatus::Stale);
    }
    if value.is_finite() && (0.0..=150.0).contains(&value) {
        Ok(value)
    } else {
        Err(ThermalStatus::Invalid)
    }
}
pub(super) fn nvml_status(error: &nvml_wrapper::error::NvmlError) -> ThermalStatus {
    use nvml_wrapper::error::NvmlError;
    match error {
        NvmlError::NotSupported | NvmlError::FunctionNotFound => ThermalStatus::Unsupported,
        NvmlError::NoPermission => ThermalStatus::PermissionDenied,
        NvmlError::GpuLost => ThermalStatus::DeviceLost,
        _ => ThermalStatus::Invalid,
    }
}

pub(super) fn valid_requests(requests: &[RmRequest]) -> bool {
    let mut bdfs = HashSet::new();
    let mut uuids = HashSet::new();
    requests.len() <= 64
        && requests.iter().all(|r| {
            !r.uuid.is_empty()
                && r.uuid.len() <= 128
                && r.driver_version.len() <= 64
                && normalize_bdf(&r.pci_bus_id)
                    .map(|b| bdfs.insert(b))
                    .unwrap_or(false)
                && uuids.insert(r.uuid.clone())
        })
}
