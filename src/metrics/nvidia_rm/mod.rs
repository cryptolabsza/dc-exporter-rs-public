//! Experimental memory-only NVIDIA RM telemetry. Call only in the isolated helper.
pub mod abi;
pub mod rusd;

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RmRequest {
    pub pci_bus_id: String,
    pub uuid: String,
    pub device_id: u16,
    pub driver_version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RmStatus {
    Valid,
    Unsupported,
    PermissionDenied,
    Invalid,
    Stale,
    Timeout,
    DeviceLost,
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RmReading {
    pub pci_bus_id: String,
    pub uuid: String,
    pub celsius: Option<f64>,
    pub status: RmStatus,
}

impl RmReading {
    pub fn unavailable(request: &RmRequest, status: RmStatus) -> Self {
        Self {
            pci_bus_id: request.pci_bus_id.clone(),
            uuid: request.uuid.clone(),
            celsius: None,
            status,
        }
    }
}

/// The parent process must enforce the deadline, including initialization and cleanup ioctls.
/// The local timeout bounds polling; it cannot interrupt a blocked driver operation.
pub fn collect_batch(
    requests: &[RmRequest],
    timeout: Duration,
    max_sample_age: Duration,
) -> Vec<RmReading> {
    if let Err(status) = rusd::validate_requests(requests) {
        return requests
            .iter()
            .map(|r| RmReading::unavailable(r, status))
            .collect();
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        rusd::native::collect(requests, timeout, max_sample_age)
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (timeout, max_sample_age);
        requests
            .iter()
            .map(|r| RmReading::unavailable(r, RmStatus::Unsupported))
            .collect()
    }
}
