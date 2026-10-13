//! GPU Metrics Collection via NVML
//!
//! Collects standard GPU metrics using NVIDIA's NVML library.
//! Uses a PERSISTENT worker thread to avoid file descriptor leaks
//! from repeatedly spawning threads and initializing NVML.
//!
//! SAFETY: Includes watchdog to detect and recover from stuck workers
//! (e.g., when GPUs are in VM passthrough or driver crash).

use std::sync::mpsc::{self, Sender, Receiver};
use std::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use std::sync::{Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{error, info, warn};

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};

use crate::state::{GpuArchitecture, GpuState, ThrottleReasons};
use super::{GpuMetrics, GpuProcess, MetricSource, ProcessType};

/// Maximum consecutive timeouts before declaring worker stuck
const MAX_CONSECUTIVE_TIMEOUTS: u32 = 3;

/// Global NVML worker manager (supports restart)
static NVML_MANAGER: RwLock<Option<NvmlWorkerManager>> = RwLock::new(None);

/// Result of NVML collection - includes status info
#[derive(Debug)]
pub struct NvmlCollectionResult {
    pub gpus: Vec<GpuMetrics>,
    pub nvml_initialized: bool,  // false = driver error, true = NVML worked
}

/// Request to the NVML worker
enum NvmlRequest {
    CollectAll(Sender<NvmlCollectionResult>),
    Ping(Sender<()>),  // Health check
    Shutdown,
}

/// Manages the NVML worker with watchdog capabilities
struct NvmlWorkerManager {
    worker: NvmlWorker,
    consecutive_timeouts: AtomicU32,
    last_success_epoch: AtomicU64,
    generation: u32,  // Incremented on each restart
}

impl NvmlWorkerManager {
    fn new(generation: u32) -> Self {
        NvmlWorkerManager {
            worker: NvmlWorker::new(),
            consecutive_timeouts: AtomicU32::new(0),
            last_success_epoch: AtomicU64::new(current_epoch_secs()),
            generation,
        }
    }
    
    fn is_stuck(&self) -> bool {
        self.consecutive_timeouts.load(Ordering::Relaxed) >= MAX_CONSECUTIVE_TIMEOUTS
    }
    
    fn record_success(&self) {
        self.consecutive_timeouts.store(0, Ordering::Relaxed);
        self.last_success_epoch.store(current_epoch_secs(), Ordering::Relaxed);
    }
    
    fn record_timeout(&self) {
        let count = self.consecutive_timeouts.fetch_add(1, Ordering::Relaxed) + 1;
        warn!("NVML worker timeout #{} (max {})", count, MAX_CONSECUTIVE_TIMEOUTS);
    }
}

/// Get current time as epoch seconds
fn current_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Persistent NVML worker that keeps a single NVML session alive
struct NvmlWorker {
    tx: Mutex<Sender<NvmlRequest>>,
    handle: Option<JoinHandle<()>>,
}

impl NvmlWorker {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel::<NvmlRequest>();
        
        let handle = thread::Builder::new()
            .name("nvml-worker".to_string())
            .spawn(move || {
                Self::worker_loop(rx);
            })
            .expect("Failed to spawn NVML worker thread");
        
        NvmlWorker {
            tx: Mutex::new(tx),
            handle: Some(handle),
        }
    }
    
    fn worker_loop(rx: Receiver<NvmlRequest>) {
        info!("NVML worker thread started");
        
        // Keep NVML initialized for the lifetime of the worker
        let (nvml, nvml_initialized) = match Nvml::init() {
            Ok(n) => {
                info!("NVML initialized successfully");
                (Some(n), true)
            }
            Err(e) => {
                warn!("Failed to initialize NVML: {:?}", e);
                (None, false)
            }
        };
        
        // Track if we ever successfully init NVML
        let mut ever_initialized = nvml_initialized;
        
        while let Ok(request) = rx.recv() {
            match request {
                NvmlRequest::CollectAll(response_tx) => {
                    let result = if let Some(ref nvml) = nvml {
                        NvmlCollectionResult {
                            gpus: collect_metrics_from_nvml(nvml),
                            nvml_initialized: true,
                        }
                    } else {
                        // Try to init again in case driver was loaded
                        match Nvml::init() {
                            Ok(nvml) => {
                                ever_initialized = true;
                                NvmlCollectionResult {
                                    gpus: collect_metrics_from_nvml(&nvml),
                                    nvml_initialized: true,
                                }
                            }
                            Err(_) => NvmlCollectionResult {
                                gpus: Vec::new(),
                                nvml_initialized: ever_initialized,
                            },
                        }
                    };
                    let _ = response_tx.send(result);
                }
                NvmlRequest::Ping(response_tx) => {
                    let _ = response_tx.send(());
                }
                NvmlRequest::Shutdown => {
                    info!("NVML worker shutting down");
                    break;
                }
            }
        }
        
        // NVML is dropped here
        info!("NVML worker thread exiting");
    }
    
    fn collect_all(&self, timeout: Duration) -> Option<NvmlCollectionResult> {
        let (response_tx, response_rx) = mpsc::channel();
        
        // Send request to worker
        {
            let tx = self.tx.lock().unwrap();
            if tx.send(NvmlRequest::CollectAll(response_tx)).is_err() {
                warn!("NVML worker channel closed");
                return None;
            }
        }
        
        // Wait for response with timeout
        match response_rx.recv_timeout(timeout) {
            Ok(result) => Some(result),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                warn!("NVML worker disconnected");
                None
            }
        }
    }
    
    /// Check if worker is responsive
    fn ping(&self, timeout: Duration) -> bool {
        let (response_tx, response_rx) = mpsc::channel();
        
        {
            let tx = self.tx.lock().unwrap();
            if tx.send(NvmlRequest::Ping(response_tx)).is_err() {
                return false;
            }
        }
        
        response_rx.recv_timeout(timeout).is_ok()
    }
    
    /// Request shutdown (non-blocking)
    fn shutdown(&self) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(NvmlRequest::Shutdown);
        }
    }
}

/// Get or create the NVML worker manager, restarting if stuck
fn get_or_restart_worker() -> Option<std::sync::RwLockReadGuard<'static, Option<NvmlWorkerManager>>> {
    // Fast path: check if manager exists and is healthy
    {
        let guard = NVML_MANAGER.read().unwrap();
        if let Some(ref manager) = *guard {
            if !manager.is_stuck() {
                return Some(guard);
            }
        }
    }
    
    // Slow path: need to create or restart
    let mut guard = NVML_MANAGER.write().unwrap();
    
    let new_generation = if let Some(ref old_manager) = *guard {
        if old_manager.is_stuck() {
            warn!("NVML worker stuck after {} consecutive timeouts - restarting (gen {})",
                  MAX_CONSECUTIVE_TIMEOUTS, old_manager.generation);
            
            // Try to shutdown old worker gracefully
            old_manager.worker.shutdown();
            
            old_manager.generation + 1
        } else {
            // Another thread fixed it, we're good
            drop(guard);
            return Some(NVML_MANAGER.read().unwrap());
        }
    } else {
        info!("Initializing NVML worker manager");
        0
    };
    
    // Create new manager
    *guard = Some(NvmlWorkerManager::new(new_generation));
    info!("NVML worker manager started (generation {})", new_generation);
    
    drop(guard);
    Some(NVML_MANAGER.read().unwrap())
}

/// Collect metrics for all available GPUs via NVML
pub struct NvmlCollector {
    timeout: Duration,
}

impl NvmlCollector {
    pub fn new(timeout: Duration) -> Self {
        // Ensure worker is initialized
        let _ = get_or_restart_worker();
        NvmlCollector { timeout }
    }

    /// Collect metrics for ALL GPUs using the persistent NVML worker
    /// Includes watchdog: if worker is stuck, it will be restarted
    /// Returns (gpus, nvml_initialized) - nvml_initialized is false if driver error
    pub fn collect_all(&self) -> NvmlCollectionResult {
        let guard = match get_or_restart_worker() {
            Some(g) => g,
            None => {
                error!("Failed to get NVML worker manager");
                return NvmlCollectionResult {
                    gpus: Vec::new(),
                    nvml_initialized: false,
                };
            }
        };
        
        let manager = match guard.as_ref() {
            Some(m) => m,
            None => {
                error!("NVML worker manager is None");
                return NvmlCollectionResult {
                    gpus: Vec::new(),
                    nvml_initialized: false,
                };
            }
        };
        
        // Use timeout * 2 for actual collection
        let timeout = self.timeout * 2;
        
        match manager.worker.collect_all(timeout) {
            Some(result) => {
                manager.record_success();
                result
            }
            None => {
                manager.record_timeout();
                
                // If now stuck, the next call will restart the worker
                if manager.is_stuck() {
                    warn!("NVML worker declared stuck - will restart on next collection");
                }
                
                NvmlCollectionResult {
                    gpus: Vec::new(),
                    nvml_initialized: false,
                }
            }
        }
    }
}

/// Collect all metrics from an existing NVML instance
fn collect_metrics_from_nvml(nvml: &Nvml) -> Vec<GpuMetrics> {
    let count = match nvml.device_count() {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to get NVML device count: {}", e);
            return Vec::new();
        }
    };

    let driver = nvml.sys_driver_version().unwrap_or_else(|_| "unknown".to_string());
    
    let mut metrics = Vec::with_capacity(count as usize);
    
    for i in 0..count {
        let device = match nvml.device_by_index(i) {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to get device {}: {}", i, e);
                continue;
            }
        };

        let mut gpu = GpuMetrics::default();
        gpu.index = i;
        gpu.source = MetricSource::Host;
        gpu.driver_version = driver.clone();

        // Basic info - if name fails, GPU has errors
        match device.name() {
            Ok(name) => {
                gpu.architecture = GpuArchitecture::from_name(&name);
                gpu.name = name;
            }
            Err(e) => {
                warn!("GPU {}: Failed to get name: {}", i, e);
                gpu.state = GpuState::Error;
                gpu.nvml_essential_query_failed = true;
                metrics.push(gpu);
                continue;
            }
        }

        if let Ok(uuid) = device.uuid() {
            gpu.uuid = uuid;
        } else {
            gpu.uuid = format!("GPU-{}", i);
        }

        if let Ok(pci) = device.pci_info() {
            gpu.pci_bus_id = format!("{:04x}:{:02x}:{:02x}.0", pci.domain, pci.bus, pci.device);
        }

        // === GPU Health Check ===
        let temp_result = device.temperature(TemperatureSensor::Gpu);
        if let Ok(temp) = temp_result {
            gpu.gpu_temp = Some(temp);
        } else {
            warn!("GPU {}: Failed to query temperature - setting error state", i);
            gpu.state = GpuState::Error;
            gpu.nvml_essential_query_failed = true;
        }

        let mem_result = device.memory_info();
        if let Ok(mem) = mem_result {
            gpu.fb_used = Some(mem.used / (1024 * 1024));
            gpu.fb_free = Some(mem.free / (1024 * 1024));
            gpu.fb_total = Some(mem.total / (1024 * 1024));
        } else {
            warn!("GPU {}: Failed to query memory info", i);
            gpu.nvml_essential_query_failed = true;
            if gpu.state == GpuState::Ok {
                gpu.state = GpuState::Warning;
            }
        }

        // ECC errors check - detailed
        if let Ok(mode) = device.is_ecc_enabled() {
            gpu.ecc_enabled = mode.currently_enabled;
        }
        
        if let Ok(errors) = device.total_ecc_errors(
            nvml_wrapper::enum_wrappers::device::MemoryError::Corrected,
            nvml_wrapper::enum_wrappers::device::EccCounter::Volatile
        ) {
            gpu.ecc_sbe = Some(errors);
        }
        
        if let Ok(errors) = device.total_ecc_errors(
            nvml_wrapper::enum_wrappers::device::MemoryError::Uncorrected,
            nvml_wrapper::enum_wrappers::device::EccCounter::Volatile
        ) {
            gpu.ecc_dbe = Some(errors);
            if errors > 0 {
                warn!("GPU {}: {} uncorrected ECC errors detected", i, errors);
                if gpu.state == GpuState::Ok {
                    gpu.state = GpuState::Warning;
                }
            }
        }

        if gpu.state == GpuState::default() {
            gpu.state = GpuState::Ok;
        }

        // Power usage and limits
        if let Ok(power_mw) = device.power_usage() {
            gpu.power_usage = Some(power_mw as f64 / 1000.0);
        }
        if let Ok(limit_mw) = device.enforced_power_limit() {
            gpu.power_limit = Some(limit_mw as f64 / 1000.0);
        }
        if let Ok(constraints) = device.power_management_limit_constraints() {
            gpu.power_max = Some(constraints.max_limit as f64 / 1000.0);
        }

        // Clocks
        if let Ok(sm) = device.clock_info(Clock::SM) {
            gpu.sm_clock = Some(sm);
        }
        if let Ok(mem) = device.clock_info(Clock::Memory) {
            gpu.mem_clock = Some(mem);
        }

        // Utilization
        if let Ok(util) = device.utilization_rates() {
            gpu.gpu_util = Some(util.gpu);
            gpu.mem_util = Some(util.memory);
        }
        
        // Encoder/Decoder utilization
        if let Ok(enc) = device.encoder_utilization() {
            gpu.encoder_util = Some(enc.utilization);
        }
        if let Ok(dec) = device.decoder_utilization() {
            gpu.decoder_util = Some(dec.utilization);
        }

        // Fan speed
        if matches!(gpu.architecture, GpuArchitecture::Datacenter) {
            gpu.fan_speed = Some(0);
        } else if let Ok(fan) = device.fan_speed(0) {
            gpu.fan_speed = Some(fan);
        } else {
            gpu.fan_speed = Some(0);
        }

        // Memory temps for datacenter GPUs
        if matches!(gpu.architecture, GpuArchitecture::Datacenter) {
            gpu.vram_temp = Some(0);
            gpu.hotspot_temp = Some(0);
        }

        // Throttle reasons
        if let Ok(reasons) = device.current_throttle_reasons() {
            gpu.throttle_reasons = ThrottleReasons::from_bitmask(reasons.bits());
        }

        // PCIe throughput (available methods in nvml-wrapper 0.10)
        if let Ok(tx) = device.pcie_throughput(nvml_wrapper::enum_wrappers::device::PcieUtilCounter::Send) {
            gpu.pcie_tx_throughput = Some(tx as u64);
        }
        if let Ok(rx) = device.pcie_throughput(nvml_wrapper::enum_wrappers::device::PcieUtilCounter::Receive) {
            gpu.pcie_rx_throughput = Some(rx as u64);
        }
        // PCIe speed (MB/s) - can be used to infer gen/width
        if let Ok(speed) = device.pcie_link_speed() {
            // Speed is in MB/s, we can estimate PCIe gen from this
            // Gen3 x16 = ~16000 MB/s, Gen4 x16 = ~32000 MB/s, Gen5 x16 = ~64000 MB/s
            let (gen, width) = estimate_pcie_gen_width(speed);
            gpu.pcie_gen = Some(gen);
            gpu.pcie_width = Some(width);
        }

        // NVLink metrics (via link_wrapper_for)
        collect_nvlink_metrics(&device, &mut gpu);
        
        // Process info
        collect_process_info(&device, &mut gpu);

        // Compute capability
        if matches!(gpu.architecture, GpuArchitecture::Unknown) {
            if let Ok(cap) = device.cuda_compute_capability() {
                let detected = GpuArchitecture::from_compute_capability(cap.major, cap.minor);
                if !matches!(detected, GpuArchitecture::Unknown) {
                    gpu.architecture = detected;
                }
            }
        }

        metrics.push(gpu);
    }

    metrics
}

/// Estimate PCIe generation and width from speed (MB/s)
fn estimate_pcie_gen_width(speed_mbps: u32) -> (u32, u32) {
    // PCIe speeds per lane: Gen3=~1GB/s, Gen4=~2GB/s, Gen5=~4GB/s
    // Common configs: x1, x4, x8, x16
    match speed_mbps {
        // Gen5
        s if s >= 60000 => (5, 16),
        s if s >= 30000 => (5, 8),
        s if s >= 15000 => (5, 4),
        // Gen4
        s if s >= 28000 => (4, 16),
        s if s >= 14000 => (4, 8),
        s if s >= 7000 => (4, 4),
        // Gen3
        s if s >= 14000 => (3, 16),
        s if s >= 7000 => (3, 8),
        s if s >= 3500 => (3, 4),
        // Gen2 or lower
        s if s >= 4000 => (2, 16),
        _ => (3, 16), // Default assumption
    }
}

/// Collect NVLink metrics for a device
fn collect_nvlink_metrics(device: &nvml_wrapper::Device, gpu: &mut GpuMetrics) {
    // NVLink is typically available on A100, H100, V100, etc.
    // Try to get NVLink info - typically up to 18 links on newer GPUs
    let max_links = 18u32;
    let mut active_links = 0u32;
    
    for link in 0..max_links {
        // Use the link_wrapper_for API to get NvLink object
        let nvlink = device.link_wrapper_for(link);
        
        // Check if link is active
        if let Ok(active) = nvlink.is_active() {
            if active {
                active_links += 1;
            }
        }
    }
    
    gpu.nvlink_active = active_links;
    gpu.nvlink_total = max_links;
    
    // Note: throughput and error counters would require additional NVML calls
    // that may not be available in nvml-wrapper 0.10
    // We just report active link count for now
}

/// Extract memory in MiB from UsedGpuMemory enum
fn get_used_memory_mib(mem: &nvml_wrapper::enums::device::UsedGpuMemory) -> u64 {
    match mem {
        nvml_wrapper::enums::device::UsedGpuMemory::Used(bytes) => bytes / (1024 * 1024),
        nvml_wrapper::enums::device::UsedGpuMemory::Unavailable => 0,
    }
}

/// Collect process information for a device
fn collect_process_info(device: &nvml_wrapper::Device, gpu: &mut GpuMetrics) {
    // Get compute processes
    if let Ok(procs) = device.running_compute_processes() {
        gpu.compute_processes = procs.len() as u32;
        
        for proc in procs.iter().take(10) {  // Limit to 10 processes
            let name = get_process_name(proc.pid);
            let used_memory = get_used_memory_mib(&proc.used_gpu_memory);
            gpu.process_list.push(GpuProcess {
                pid: proc.pid,
                name,
                used_memory,
                process_type: ProcessType::Compute,
            });
        }
    }
    
    // Get graphics processes
    if let Ok(procs) = device.running_graphics_processes() {
        gpu.graphics_processes = procs.len() as u32;
        
        for proc in procs.iter().take(10) {
            let used_memory = get_used_memory_mib(&proc.used_gpu_memory);
            // Check if we already have this PID (mixed compute/graphics)
            let existing = gpu.process_list.iter_mut().find(|p| p.pid == proc.pid);
            if let Some(existing) = existing {
                existing.process_type = ProcessType::Mixed;
                // Add memory usage
                existing.used_memory += used_memory;
            } else {
                let name = get_process_name(proc.pid);
                gpu.process_list.push(GpuProcess {
                    pid: proc.pid,
                    name,
                    used_memory,
                    process_type: ProcessType::Graphics,
                });
            }
        }
    }
    
    // Sort by memory usage descending
    gpu.process_list.sort_by(|a, b| b.used_memory.cmp(&a.used_memory));
}

/// Get process name from PID
fn get_process_name(pid: u32) -> String {
    // Try to read from /proc/<pid>/comm
    if let Ok(name) = std::fs::read_to_string(format!("/proc/{}/comm", pid)) {
        return name.trim().to_string();
    }
    
    // Fallback: try cmdline
    if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{}/cmdline", pid)) {
        if let Some(cmd) = cmdline.split('\0').next() {
            if let Some(name) = cmd.rsplit('/').next() {
                return name.to_string();
            }
        }
    }
    
    format!("pid:{}", pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collector_creation() {
        let collector = NvmlCollector::new(Duration::from_secs(5));
        // Just verify it doesn't panic
        let _ = collector.collect_all();
    }
}
