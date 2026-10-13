//! Metrics collection modules
//!
//! - collector: Main metrics collection orchestrator
//! - gpu: GPU metrics via NVML
//! - pci: PCIe bus scanning
//! - register: Direct register access for VRAM/hotspot temps
//! - vm: VM GPU metrics via QEMU guest agent
//! - prometheus: Prometheus metrics formatting

pub mod collector;
pub mod dmesg;
pub mod gpu;
pub mod pci;
pub mod prometheus;
pub mod register;
pub mod vm;
pub mod nvidia_rm;
pub mod thermal;
pub mod thermal_worker;

pub use gpu::NvmlCollectionResult;
pub use register::RegisterAccessStatus;

use crate::state::{GpuArchitecture, GpuState, ThrottleReasons};

/// Complete metrics for a single GPU
#[derive(Debug, Clone, Default)]
pub struct GpuMetrics {
    /// GPU index (consistent across host and VMs)
    pub index: u32,
    
    /// GPU name (e.g., "NVIDIA GeForce RTX 4090")
    pub name: String,
    
    /// GPU UUID
    pub uuid: String,
    
    /// PCI bus ID (e.g., "0000:01:00.0")
    pub pci_bus_id: String,
    /// PCI device ID from the reconciled host inventory, never an enumeration index.
    pub device_id: u16,
    
    /// Driver version
    pub driver_version: String,
    
    /// GPU state (OK, Warning, Error, VmPassthrough, Unavailable)
    pub state: GpuState,

    /// Whether a required NVML telemetry query failed during this collection.
    /// Identity fields may still be usable, but operational telemetry is not trustworthy.
    pub nvml_essential_query_failed: bool,
    
    /// GPU architecture type
    pub architecture: GpuArchitecture,
    
    // Temperatures
    pub gpu_temp: Option<u32>,
    pub vram_temp: Option<u32>,
    pub hotspot_temp: Option<u32>,
    pub vram_temp_approximated: bool,
    pub hotspot_temp_approximated: bool,
    /// VRAM backend and validity, separate from host/guest source classification.
    pub vram_thermal: Option<thermal::ThermalMetadata>,
    
    // Power and clocks
    pub power_usage: Option<f64>,   // Watts
    pub power_limit: Option<f64>,   // Watts (enforced limit)
    pub power_max: Option<f64>,     // Watts (max supported)
    pub sm_clock: Option<u32>,      // MHz
    pub mem_clock: Option<u32>,     // MHz
    
    // Utilization
    pub gpu_util: Option<u32>,      // Percent
    pub mem_util: Option<u32>,      // Percent
    pub encoder_util: Option<u32>,  // Percent (NVENC)
    pub decoder_util: Option<u32>,  // Percent (NVDEC)
    
    // Memory
    pub fb_used: Option<u64>,       // MiB
    pub fb_free: Option<u64>,       // MiB
    pub fb_total: Option<u64>,      // MiB
    
    // Fan
    pub fan_speed: Option<u32>,     // Percent
    
    // Throttle reasons
    pub throttle_reasons: ThrottleReasons,
    
    // PCIe info
    pub pcie_gen: Option<u32>,          // Current PCIe generation
    pub pcie_gen_max: Option<u32>,      // Max supported generation
    pub pcie_width: Option<u32>,        // Current link width
    pub pcie_width_max: Option<u32>,    // Max supported width
    pub pcie_tx_throughput: Option<u64>, // KB/s
    pub pcie_rx_throughput: Option<u64>, // KB/s
    
    // ECC errors (datacenter GPUs)
    pub ecc_sbe: Option<u64>,       // Single-bit errors (correctable)
    pub ecc_dbe: Option<u64>,       // Double-bit errors (uncorrectable)
    pub ecc_enabled: bool,
    
    // NVLink (multi-GPU systems)
    pub nvlink_active: u32,         // Number of active NVLinks
    pub nvlink_total: u32,          // Total NVLink connections
    pub nvlink_tx_throughput: Option<u64>, // Total TX KB/s across all links
    pub nvlink_rx_throughput: Option<u64>, // Total RX KB/s across all links
    pub nvlink_errors: u64,         // Total errors across all links
    
    // Processes
    pub compute_processes: u32,     // Number of compute processes
    pub graphics_processes: u32,    // Number of graphics processes
    pub process_list: Vec<GpuProcess>, // Detailed process list
    
    // AER errors (PCIe)
    pub aer_errors: u32,
    
    // Hardware failure detection
    /// Reason for hardware failure (dmesg, /proc health, etc.)
    pub hardware_failure_reason: Option<String>,
    /// Whether this GPU has confirmed hardware failure from dmesg
    pub dmesg_hardware_failure: bool,
    /// RmInitAdapter failure count from dmesg
    pub dmesg_rm_init_failed: u32,
    /// Whether the GPU has fallen off the PCIe bus
    pub fallen_off_bus: bool,
    /// XID error codes from dmesg
    pub xid_errors: Vec<u32>,
    /// Driver bound to this PCI device (nvidia, vfio-pci)
    pub bound_driver: Option<String>,
    
    // Source (for debugging)
    pub source: MetricSource,
}

/// Information about a process using the GPU
#[derive(Debug, Clone, Default)]
pub struct GpuProcess {
    pub pid: u32,
    pub name: String,
    pub used_memory: u64,  // MiB
    pub process_type: ProcessType,
}

/// Type of GPU process
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProcessType {
    #[default]
    Compute,
    Graphics,
    Mixed,
}

/// Source of the metrics
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MetricSource {
    #[default]
    Host,       // Collected from host via NVML
    Register,   // Collected via direct register access
    Vm,         // Collected from inside VM
    Pci,        // Only PCIe info available (passthrough)
}

impl MetricSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            MetricSource::Host => "host",
            MetricSource::Register => "register",
            MetricSource::Vm => "vm",
            MetricSource::Pci => "pci",
        }
    }
}

/// Collection of all metrics
#[derive(Debug, Clone, Default)]
pub struct AllMetrics {
    /// Hostname
    pub hostname: String,
    
    /// Driver version (if available)
    pub driver_version: String,
    
    /// Total GPUs detected on PCIe bus
    pub total_pci_gpus: u32,
    
    /// Successfully constructed per-GPU NVML records for the current collection
    pub nvml_gpus: u32,
    
    /// Whether any GPUs are available to host
    pub gpus_available: bool,
    
    /// Individual GPU metrics
    pub gpus: Vec<GpuMetrics>,
    
    /// APT upgradable packages count
    pub apt_upgradable: Option<u32>,
    
    /// Collection timestamp (Unix epoch)
    pub timestamp: u64,
}
