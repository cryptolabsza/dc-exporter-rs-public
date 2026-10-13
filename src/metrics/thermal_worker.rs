//! TRACE-THERMAL-006/007: bounded one-batch helper with persistent parent ownership.
//! A kill request never counts as child exit; only try_wait permits replacement.
use super::nvidia_rm::{RmReading, RmRequest, RmStatus};
use super::thermal::{
    decode_nvml_sample, normalize_bdf, nvml_status, valid_requests, ThermalBackend, ThermalStatus,
};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_FRAME: usize = 64 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerError {
    Timeout,
    Busy,
    Invalid,
    Io,
    Unsupported,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchRequest {
    request_id: u64,
    requests: Vec<RmRequest>,
    timeout_ms: u64,
    max_sample_age_ms: u64,
    backend: ThermalBackend,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchResponse {
    request_id: u64,
    readings: Vec<ThermalReading>,
}
fn rusd_default() -> ThermalBackend {
    ThermalBackend::Rusd
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalReading {
    #[serde(default = "rusd_default")]
    pub backend: ThermalBackend,
    #[serde(flatten)]
    pub reading: RmReading,
    /// Child's remaining freshness budget after batch collection and driver cleanup.
    /// Required for valid wire readings; no default freshness is inferred.
    #[serde(default)]
    pub remaining_validity_us: Option<u64>,
    /// Actual NVML field timestamp, with units documented by the pinned wrapper.
    #[serde(skip)]
    pub sample_timestamp_us: Option<i64>,
    /// Conservative RUSD age bound begins before the entire RM collection call.
    #[serde(skip)]
    pub age_started_at: Option<Instant>,
}
impl std::ops::Deref for ThermalReading {
    type Target = RmReading;
    fn deref(&self) -> &RmReading {
        &self.reading
    }
}
struct WorkerState {
    child: Option<Child>,
    next_id: u64,
    max_live_children: usize,
}
pub struct ThermalManager {
    executable: PathBuf,
    args: Vec<String>,
    state: Mutex<WorkerState>,
}
impl Default for ThermalManager {
    fn default() -> Self {
        Self::with_command(
            std::env::current_exe().unwrap_or_default(),
            vec!["--thermal-helper".into()],
        )
    }
}
impl ThermalManager {
    pub fn with_command(executable: PathBuf, args: Vec<String>) -> Self {
        Self {
            executable,
            args,
            state: Mutex::new(WorkerState {
                child: None,
                next_id: 0,
                max_live_children: 0,
            }),
        }
    }
    pub fn max_live_children(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .max_live_children
    }
    pub fn collect(
        &self,
        requests: &[RmRequest],
        timeout: Duration,
        max_sample_age: Duration,
    ) -> Result<Vec<ThermalReading>, WorkerError> {
        self.collect_with_backend(requests, timeout, max_sample_age, ThermalBackend::Rusd)
    }
    pub fn collect_with_backend(
        &self,
        requests: &[RmRequest],
        timeout: Duration,
        max_sample_age: Duration,
        backend: ThermalBackend,
    ) -> Result<Vec<ThermalReading>, WorkerError> {
        let batch_started = Instant::now();
        if !valid_requests(requests)
            || timeout.is_zero()
            || timeout > Duration::from_secs(60)
            || max_sample_age.is_zero()
            || !matches!(
                backend,
                ThermalBackend::Auto | ThermalBackend::Nvml | ThermalBackend::Rusd
            )
        {
            return Err(WorkerError::Invalid);
        }
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let deadline = batch_started + timeout;
        let mut state = match self.state.try_lock() {
            Ok(s) => s,
            Err(TryLockError::WouldBlock) => return Err(WorkerError::Busy),
            Err(_) => return Err(WorkerError::Io),
        };
        if let Some(child) = state.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => state.child = None,
                Ok(None) => return Err(WorkerError::Busy),
                Err(_) => return Err(WorkerError::Io),
            }
        }
        state.next_id = state.next_id.checked_add(1).ok_or(WorkerError::Invalid)?;
        let batch = BatchRequest {
            request_id: state.next_id,
            requests: requests.to_vec(),
            timeout_ms: timeout
                .as_millis()
                .try_into()
                .map_err(|_| WorkerError::Invalid)?,
            max_sample_age_ms: max_sample_age
                .as_millis()
                .try_into()
                .map_err(|_| WorkerError::Invalid)?,
            backend,
        };
        let frame = serde_json::to_vec(&batch).map_err(|_| WorkerError::Invalid)?;
        if frame.len() > MAX_FRAME {
            return Err(WorkerError::Invalid);
        }
        // No pipes are left in blocking mode. No cleanup thread can accumulate.
        let child = Command::new(&self.executable)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| WorkerError::Io)?;
        state.child = Some(child);
        state.max_live_children = state.max_live_children.max(1);
        let result = (|| {
            let child = state.child.as_mut().unwrap();
            let mut input = child.stdin.take().ok_or(WorkerError::Io)?;
            let mut output = child.stdout.take().ok_or(WorkerError::Io)?;
            nonblocking(&input)?;
            nonblocking(&output)?;
            write_deadline(&mut input, &(frame.len() as u32).to_le_bytes(), deadline)?;
            write_deadline(&mut input, &frame, deadline)?;
            drop(input);
            let data = read_frame(&mut output, deadline)?;
            let mut reply: BatchResponse =
                serde_json::from_slice(&data).map_err(|_| WorkerError::Invalid)?;
            if reply.request_id != batch.request_id || reply.readings.len() != requests.len() {
                return Err(WorkerError::Invalid);
            }
            let mut seen = std::collections::HashSet::new();
            for r in &reply.readings {
                let bdf = normalize_bdf(&r.pci_bus_id).ok_or(WorkerError::Invalid)?;
                if !seen.insert(bdf.clone())
                    || !requests.iter().any(|q| {
                        normalize_bdf(&q.pci_bus_id) == Some(bdf.clone()) && q.uuid == r.uuid
                    })
                    || !matches!(r.backend, ThermalBackend::Nvml | ThermalBackend::Rusd)
                    || (backend != ThermalBackend::Auto && r.backend != backend)
                    || (r.status == RmStatus::Valid
                        && (!r
                            .celsius
                            .map(|t| t.is_finite() && (0.0..=150.0).contains(&t))
                            .unwrap_or(false)
                            || !r
                                .remaining_validity_us
                                .map(|budget| u128::from(budget) <= max_sample_age.as_micros())
                                .unwrap_or(false)))
                {
                    return Err(WorkerError::Invalid);
                }
            }
            // The single response follows RM cleanup. Require successful process exit too.
            loop {
                check_deadline(deadline)?;
                if let Some(status) = child.try_wait().map_err(|_| WorkerError::Io)? {
                    if !status.success() {
                        return Err(WorkerError::Invalid);
                    }
                    let mut extra = [0u8; 1];
                    match output.read(&mut extra) {
                        Ok(0) => {}
                        _ => return Err(WorkerError::Invalid),
                    }
                    // Charge the entire parent round trip, including serialization and
                    // successful child exit, against the child's remaining budget.
                    let elapsed = batch_started.elapsed().as_micros();
                    for r in &mut reply.readings {
                        if r.status == RmStatus::Valid {
                            let budget = u128::from(r.remaining_validity_us.unwrap());
                            if elapsed >= budget {
                                r.reading.status = RmStatus::Stale;
                                r.reading.celsius = None;
                                r.remaining_validity_us = Some(0);
                            } else {
                                r.remaining_validity_us = Some((budget - elapsed) as u64);
                            }
                        }
                    }
                    return Ok(reply.readings);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        })();
        if result.is_err() {
            if let Some(child) = state.child.as_mut() {
                let _ = child.kill();
            }
        }
        // Never wait(): an unexited or kernel-stuck child stays owned for the next batch.
        if let Some(child) = state.child.as_mut() {
            if matches!(child.try_wait(), Ok(Some(_))) {
                state.child = None;
            }
        }
        result
    }
}
impl Drop for ThermalManager {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut() {
            if let Some(child) = state.child.as_mut() {
                let _ = child.kill();
                let _ = child.try_wait();
            }
        }
    }
}
fn check_deadline(deadline: Instant) -> Result<(), WorkerError> {
    if Instant::now() >= deadline {
        Err(WorkerError::Timeout)
    } else {
        Ok(())
    }
}
#[cfg(unix)]
fn nonblocking<T: std::os::fd::AsRawFd>(pipe: &T) -> Result<(), WorkerError> {
    let fd = pipe.as_raw_fd();
    // SAFETY: fcntl only changes flags on a live, owned pipe descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        Err(WorkerError::Io)
    } else {
        Ok(())
    }
}
#[cfg(not(unix))]
fn nonblocking<T>(_pipe: &T) -> Result<(), WorkerError> {
    Err(WorkerError::Unsupported)
}
fn write_deadline<W: Write>(
    pipe: &mut W,
    mut data: &[u8],
    deadline: Instant,
) -> Result<(), WorkerError> {
    while !data.is_empty() {
        check_deadline(deadline)?;
        match pipe.write(data) {
            Ok(0) => return Err(WorkerError::Io),
            Ok(n) => data = &data[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(_) => return Err(WorkerError::Io),
        }
    }
    Ok(())
}
fn read_deadline<R: Read>(
    pipe: &mut R,
    mut data: &mut [u8],
    deadline: Instant,
) -> Result<(), WorkerError> {
    while !data.is_empty() {
        check_deadline(deadline)?;
        match pipe.read(data) {
            Ok(0) => return Err(WorkerError::Invalid),
            Ok(n) => data = &mut data[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(_) => return Err(WorkerError::Io),
        }
    }
    Ok(())
}
fn read_frame<R: Read>(pipe: &mut R, deadline: Instant) -> Result<Vec<u8>, WorkerError> {
    let mut header = [0u8; 4];
    read_deadline(pipe, &mut header, deadline)?;
    let len = u32::from_le_bytes(header) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(WorkerError::Invalid);
    }
    let mut body = vec![0; len];
    read_deadline(pipe, &mut body, deadline)?;
    Ok(body)
}

/// Internal child entry point. Runs before normal exporter startup or device checks.
pub fn run_helper() -> anyhow::Result<()> {
    let mut input = std::io::stdin();
    let mut output = std::io::stdout();
    nonblocking(&input).map_err(|e| anyhow::anyhow!("helper input: {e:?}"))?;
    nonblocking(&output).map_err(|e| anyhow::anyhow!("helper output: {e:?}"))?;
    let start = Instant::now();
    let data = read_frame(&mut input, start + Duration::from_secs(2))
        .map_err(|e| anyhow::anyhow!("helper frame: {e:?}"))?;
    let request: BatchRequest = serde_json::from_slice(&data)?;
    anyhow::ensure!(
        valid_requests(&request.requests)
            && (1..=60_000).contains(&request.timeout_ms)
            && request.max_sample_age_ms > 0
            && matches!(
                request.backend,
                ThermalBackend::Auto | ThermalBackend::Nvml | ThermalBackend::Rusd
            ),
        "invalid helper request"
    );
    let deadline = start + Duration::from_millis(request.timeout_ms);
    let age = Duration::from_millis(request.max_sample_age_ms);
    let mut readings = collect_child(&request.requests, request.backend, deadline, age);
    // NVML shutdown and any RUSD cleanup have completed before this final child check.
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_micros()
        .try_into()
        .map_err(|_| anyhow::anyhow!("helper clock out of range"))?;
    finalize_child_readings(&mut readings, age, now_us, Instant::now());
    let data = serde_json::to_vec(&BatchResponse {
        request_id: request.request_id,
        readings,
    })?;
    anyhow::ensure!(data.len() <= MAX_FRAME, "helper response too large");
    write_deadline(&mut output, &(data.len() as u32).to_le_bytes(), deadline)
        .map_err(|e| anyhow::anyhow!("helper output: {e:?}"))?;
    write_deadline(&mut output, &data, deadline)
        .map_err(|e| anyhow::anyhow!("helper output: {e:?}"))?;
    loop {
        check_deadline(deadline).map_err(|e| anyhow::anyhow!("helper flush: {e:?}"))?;
        match output.flush() {
            Ok(()) => break,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn reading(
    request: &RmRequest,
    backend: ThermalBackend,
    status: RmStatus,
    celsius: Option<f64>,
) -> ThermalReading {
    ThermalReading {
        backend,
        remaining_validity_us: None,
        sample_timestamp_us: None,
        age_started_at: None,
        reading: RmReading {
            pci_bus_id: request.pci_bus_id.clone(),
            uuid: request.uuid.clone(),
            status,
            celsius,
        },
    }
}
fn collect_child(
    requests: &[RmRequest],
    backend: ThermalBackend,
    deadline: Instant,
    age: Duration,
) -> Vec<ThermalReading> {
    if backend == ThermalBackend::Rusd {
        let age_started_at = Instant::now();
        return super::nvidia_rm::collect_batch(
            requests,
            deadline.saturating_duration_since(Instant::now()),
            age,
        )
        .into_iter()
        .map(|reading| ThermalReading {
            backend: ThermalBackend::Rusd,
            reading,
            remaining_validity_us: None,
            sample_timestamp_us: None,
            age_started_at: Some(age_started_at),
        })
        .collect();
    }
    let mut results = collect_nvml(requests, deadline, age);
    if backend == ThermalBackend::Auto {
        let fallback = rusd_fallback_requests(requests, &results);
        if !fallback.is_empty() && Instant::now() < deadline {
            let age_started_at = Instant::now();
            for rm in super::nvidia_rm::collect_batch(
                &fallback,
                deadline.saturating_duration_since(Instant::now()),
                age,
            ) {
                if let Some(slot) = results.iter_mut().find(|r| {
                    r.uuid == rm.uuid
                        && normalize_bdf(&r.pci_bus_id) == normalize_bdf(&rm.pci_bus_id)
                }) {
                    *slot = ThermalReading {
                        backend: ThermalBackend::Rusd,
                        reading: rm,
                        remaining_validity_us: None,
                        sample_timestamp_us: None,
                        age_started_at: Some(age_started_at),
                    };
                }
            }
        }
    }
    results
}

fn rusd_fallback_requests(requests: &[RmRequest], results: &[ThermalReading]) -> Vec<RmRequest> {
    requests
        .iter()
        .filter(|request| {
            results.iter().any(|r| {
                r.uuid == request.uuid
                    && normalize_bdf(&r.pci_bus_id) == normalize_bdf(&request.pci_bus_id)
                    && r.status == RmStatus::Unsupported
            })
        })
        .cloned()
        .collect()
}
fn as_rm(status: ThermalStatus) -> RmStatus {
    match status {
        ThermalStatus::Valid => RmStatus::Valid,
        ThermalStatus::Unsupported => RmStatus::Unsupported,
        ThermalStatus::PermissionDenied => RmStatus::PermissionDenied,
        ThermalStatus::Invalid => RmStatus::Invalid,
        ThermalStatus::Stale => RmStatus::Stale,
        ThermalStatus::Timeout => RmStatus::Timeout,
        ThermalStatus::DeviceLost => RmStatus::DeviceLost,
        ThermalStatus::Disabled => RmStatus::Disabled,
    }
}
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn collect_nvml(requests: &[RmRequest], deadline: Instant, age: Duration) -> Vec<ThermalReading> {
    use nvml_wrapper::{structs::device::FieldId, Nvml};
    let nvml = match Nvml::init() {
        Ok(n) => n,
        Err(e) => {
            return requests
                .iter()
                .map(|r| reading(r, ThermalBackend::Nvml, as_rm(nvml_status(&e)), None))
                .collect()
        }
    };
    requests
        .iter()
        .map(|request| {
            let result = (|| {
                if Instant::now() >= deadline {
                    return Err(ThermalStatus::Timeout);
                }
                let bdf = normalize_bdf(&request.pci_bus_id).ok_or(ThermalStatus::Invalid)?;
                let driver = std::fs::read_link(format!("/sys/bus/pci/devices/{bdf}/driver"))
                    .map_err(|_| ThermalStatus::DeviceLost)?;
                if driver.file_name().and_then(|s| s.to_str()) != Some("nvidia") {
                    return Err(ThermalStatus::DeviceLost);
                }
                let device = nvml
                    .device_by_pci_bus_id(bdf.as_str())
                    .map_err(|e| nvml_status(&e))?;
                if device.uuid().map_err(|e| nvml_status(&e))? != request.uuid
                    || normalize_bdf(&device.pci_info().map_err(|e| nvml_status(&e))?.bus_id)
                        != Some(bdf)
                {
                    return Err(ThermalStatus::DeviceLost);
                }
                let samples = device
                    .field_values_for(&[FieldId(
                        nvml_wrapper::sys_exports::field_id::NVML_FI_DEV_MEMORY_TEMP,
                    )])
                    .map_err(|e| nvml_status(&e))?;
                if samples.len() != 1 {
                    return Err(ThermalStatus::Invalid);
                }
                let sample = samples
                    .into_iter()
                    .next()
                    .unwrap()
                    .map_err(|e| nvml_status(&e))?;
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| ThermalStatus::Invalid)?
                    .as_micros()
                    .try_into()
                    .map_err(|_| ThermalStatus::Invalid)?;
                decode_nvml_sample(&sample, now, age).map(|t| (t, sample.timestamp))
            })();
            match result {
                Ok((t, timestamp)) => {
                    let mut result =
                        reading(request, ThermalBackend::Nvml, RmStatus::Valid, Some(t));
                    result.sample_timestamp_us = Some(timestamp);
                    result
                }
                Err(s) => reading(request, ThermalBackend::Nvml, as_rm(s), None),
            }
        })
        .collect::<Vec<_>>()
}

fn finalize_child_readings(
    readings: &mut [ThermalReading],
    max_age: Duration,
    now_us: i64,
    now: Instant,
) {
    for result in readings {
        if result.status != RmStatus::Valid {
            continue;
        }
        let elapsed = match result.backend {
            ThermalBackend::Nvml => result
                .sample_timestamp_us
                .filter(|timestamp| *timestamp > 0 && *timestamp <= now_us)
                .map(|timestamp| (now_us - timestamp) as u128),
            ThermalBackend::Rusd => result
                .age_started_at
                .and_then(|start| now.checked_duration_since(start))
                .map(|age| age.as_micros()),
            _ => None,
        };
        match elapsed {
            Some(age) if age < max_age.as_micros() => {
                result.remaining_validity_us =
                    Some((max_age.as_micros() - age).min(u64::MAX as u128) as u64);
            }
            Some(_) => {
                result.reading.status = RmStatus::Stale;
                result.reading.celsius = None;
                result.remaining_validity_us = Some(0);
            }
            None => {
                result.reading.status = RmStatus::Invalid;
                result.reading.celsius = None;
                result.remaining_validity_us = None;
            }
        }
    }
}
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn collect_nvml(requests: &[RmRequest], _deadline: Instant, _age: Duration) -> Vec<ThermalReading> {
    requests
        .iter()
        .map(|r| reading(r, ThermalBackend::Nvml, RmStatus::Unsupported, None))
        .collect()
}

#[cfg(test)]
mod freshness_tests {
    use super::*;

    #[test]
    fn second_device_and_cleanup_expire_prior_nvml_and_rusd_before_encoding() {
        let start = Instant::now();
        let request = RmRequest {
            pci_bus_id: "0000:23:00.0".into(),
            uuid: "GPU-A".into(),
            device_id: 0x2b85,
            driver_version: "590.48.01".into(),
        };
        let mut nvml = reading(&request, ThermalBackend::Nvml, RmStatus::Valid, Some(72.0));
        nvml.sample_timestamp_us = Some(1_000_000);
        let mut rusd = reading(&request, ThermalBackend::Rusd, RmStatus::Valid, Some(72.0));
        rusd.age_started_at = Some(start);
        let mut readings = vec![nvml, rusd];
        finalize_child_readings(
            &mut readings,
            Duration::from_millis(100),
            1_150_000,
            start + Duration::from_millis(150),
        );
        for r in &readings {
            assert_eq!(r.status, RmStatus::Stale);
            assert_eq!(r.celsius, None);
        }
    }

    #[test]
    fn remaining_budget_uses_real_nvml_timestamp_or_conservative_rusd_elapsed_time() {
        let start = Instant::now();
        let request = RmRequest {
            pci_bus_id: "0000:23:00.0".into(),
            uuid: "GPU-A".into(),
            device_id: 0x2b85,
            driver_version: "590.48.01".into(),
        };
        let mut nvml = reading(&request, ThermalBackend::Nvml, RmStatus::Valid, Some(0.0));
        nvml.sample_timestamp_us = Some(1_000_000);
        let mut rusd = reading(&request, ThermalBackend::Rusd, RmStatus::Valid, Some(0.0));
        rusd.age_started_at = Some(start);
        let mut readings = vec![nvml, rusd];
        finalize_child_readings(
            &mut readings,
            Duration::from_millis(100),
            1_020_000,
            start + Duration::from_millis(20),
        );
        for r in &readings {
            assert_eq!(r.status, RmStatus::Valid);
            assert_eq!(r.remaining_validity_us, Some(80000));
        }
    }

    #[test]
    fn actual_nvml_error_samples_select_only_unsupported_for_rusd_fallback() {
        use nvml_wrapper::{
            error::NvmlError, struct_wrappers::device::FieldValueSample, structs::device::FieldId,
        };
        let request = RmRequest {
            pci_bus_id: "0000:23:00.0".into(),
            uuid: "GPU-A".into(),
            device_id: 0x2b85,
            driver_version: "590.48.01".into(),
        };
        for timestamp in [0, 1_000_000] {
            for (error, expected) in [
                (NvmlError::NotSupported, 1),
                (NvmlError::FunctionNotFound, 1),
                (NvmlError::NoPermission, 0),
                (NvmlError::GpuLost, 0),
            ] {
                let sample = FieldValueSample {
                    field: FieldId(82),
                    timestamp,
                    latency: 0,
                    value: Err(error),
                };
                let status = as_rm(
                    decode_nvml_sample(&sample, 10_000_000, Duration::from_secs(3)).unwrap_err(),
                );
                let readings = vec![reading(&request, ThermalBackend::Nvml, status, None)];
                let fallback = rusd_fallback_requests(std::slice::from_ref(&request), &readings);
                assert_eq!(fallback.len(), expected);
                if expected == 1 {
                    assert_eq!(fallback[0].pci_bus_id, request.pci_bus_id);
                    assert_eq!(fallback[0].uuid, request.uuid);
                }
            }
        }
    }
}
