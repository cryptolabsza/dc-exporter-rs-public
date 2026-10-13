//! GPU State Machine
//!
//! Tracks the state of each GPU, handling transitions between:
//! - OK: GPU accessible and healthy
//! - Warning: GPU has warnings (ECC errors, high temp)
//! - Error: GPU has errors, can't query properly
//! - VmPassthrough: GPU visible on PCIe but not in NVML (passed to VM)
//! - Unavailable: GPU not responding at all

use std::fmt;

/// GPU state enumeration
/// Dashboard mapping: 0=unknown, 1=no error, 2=error
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GpuState {
    #[default]
    Ok,
    Warning,
    Error,
    VmPassthrough,
    Unavailable,
    /// GPU is on PCIe bus and bound to nvidia driver, but has confirmed hardware failure
    /// (RmInitAdapter failed, broken VBIOS, XID errors, fallen off bus)
    /// This is distinct from VmPassthrough (vfio-pci) and Error (generic driver issue)
    HardwareFailure,
}

impl GpuState {
    /// Get the numeric value for GPU_ERROR_STATE metric
    /// Dashboard expects: 0=unknown, 1=no error, 2=error, 3=vm_passthrough, 5=hw_failure
    pub fn as_u8(&self) -> u8 {
        match self {
            GpuState::Ok => 1,              // no error
            GpuState::Warning => 1,         // no error (warning is not an error)
            GpuState::Error => 2,           // error
            GpuState::VmPassthrough => 3,   // VM passthrough (special state)
            GpuState::Unavailable => 0,     // unknown state
            GpuState::HardwareFailure => 5, // confirmed hardware failure (needs RMA)
        }
    }

    /// Get a human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            GpuState::Ok => "GPU accessible and healthy",
            GpuState::Warning => "GPU has warnings (ECC errors, high temp)",
            GpuState::Error => "GPU has errors",
            GpuState::VmPassthrough => "GPU passed through to VM",
            GpuState::Unavailable => "GPU not responding",
            GpuState::HardwareFailure => "GPU hardware failure (RmInitAdapter failed, broken VBIOS, or fallen off bus)",
        }
    }
}

impl fmt::Display for GpuState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// GPU architecture type for determining temperature reading method
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GpuArchitecture {
    #[default]
    Unknown,
    /// Consumer GPUs (GeForce RTX 30xx/40xx) - use register offsets
    Consumer,
    /// Datacenter GPUs (A100/H100/A30) - use NVML field values
    Datacenter,
    /// Professional GPUs (RTX A6000, Quadro) - try both methods
    Professional,
}

impl GpuArchitecture {
    /// Detect architecture from GPU name
    pub fn from_name(name: &str) -> Self {
        // Datacenter GPUs (A100, H100, A30, etc.)
        if name.contains("A100")
            || name.contains("H100")
            || name.contains("H200")
            || name.contains("A30")
            || name.contains("A40")
            || name.contains("A10")
            || name.contains("L40")
            || name.contains("L4")
        {
            return GpuArchitecture::Datacenter;
        }

        // Professional GPUs (RTX A6000, A5000, RTX 5000/4500/4000 Ada, etc.)
        if name.contains("RTX A") 
            || name.contains("Quadro")
            || name.contains("RTX 5000")
            || name.contains("RTX 4500") 
            || name.contains("RTX 4000")
            || name.contains("RTX 6000")
        {
            return GpuArchitecture::Professional;
        }

        // Consumer GPUs (GeForce RTX 30xx/40xx/50xx)
        if name.contains("RTX 30")
            || name.contains("RTX 40")
            || name.contains("RTX 50")
            || name.contains("RTX 3060")
            || name.contains("RTX 3070")
            || name.contains("RTX 3080")
            || name.contains("RTX 3090")
            || name.contains("RTX 4060")
            || name.contains("RTX 4070")
            || name.contains("RTX 4080")
            || name.contains("RTX 4090")
            || name.contains("RTX 5070")
            || name.contains("RTX 5080")
            || name.contains("RTX 5090")
            || name.contains("GeForce")
        {
            return GpuArchitecture::Consumer;
        }

        GpuArchitecture::Unknown
    }

    /// Detect from compute capability
    pub fn from_compute_capability(major: i32, _minor: i32) -> Self {
        if major >= 9 {
            // Hopper and later
            GpuArchitecture::Datacenter
        } else if major >= 8 {
            // Ampere/Ada - could be consumer or datacenter
            GpuArchitecture::Unknown
        } else {
            GpuArchitecture::Unknown
        }
    }

    /// Should we use direct register access for VRAM temps?
    pub fn use_register_access(&self) -> bool {
        matches!(self, GpuArchitecture::Consumer | GpuArchitecture::Professional)
    }

    /// Should we use NVML field values for memory temps?
    pub fn use_nvml_field_values(&self) -> bool {
        matches!(self, GpuArchitecture::Datacenter | GpuArchitecture::Professional)
    }
}

/// Throttle reason flags
#[derive(Clone, Copy, Debug, Default)]
pub struct ThrottleReasons {
    pub gpu_idle: bool,
    pub applications_clocks_setting: bool,
    pub sw_power_cap: bool,
    pub hw_slowdown: bool,
    pub sync_boost: bool,
    pub sw_thermal_slowdown: bool,
    pub hw_thermal_slowdown: bool,
    pub hw_power_brake_slowdown: bool,
    pub display_clock_setting: bool,
}

impl ThrottleReasons {
    /// Parse from NVML throttle reason bitmask
    pub fn from_bitmask(mask: u64) -> Self {
        // NVML throttle reason bit values
        const GPU_IDLE: u64 = 0x0000000000000001;
        const APPLICATIONS_CLOCKS_SETTING: u64 = 0x0000000000000002;
        const SW_POWER_CAP: u64 = 0x0000000000000004;
        const HW_SLOWDOWN: u64 = 0x0000000000000008;
        const SYNC_BOOST: u64 = 0x0000000000000010;
        const SW_THERMAL_SLOWDOWN: u64 = 0x0000000000000020;
        const HW_THERMAL_SLOWDOWN: u64 = 0x0000000000000040;
        const HW_POWER_BRAKE_SLOWDOWN: u64 = 0x0000000000000080;
        const DISPLAY_CLOCK_SETTING: u64 = 0x0000000000000100;

        ThrottleReasons {
            gpu_idle: mask & GPU_IDLE != 0,
            applications_clocks_setting: mask & APPLICATIONS_CLOCKS_SETTING != 0,
            sw_power_cap: mask & SW_POWER_CAP != 0,
            hw_slowdown: mask & HW_SLOWDOWN != 0,
            sync_boost: mask & SYNC_BOOST != 0,
            sw_thermal_slowdown: mask & SW_THERMAL_SLOWDOWN != 0,
            hw_thermal_slowdown: mask & HW_THERMAL_SLOWDOWN != 0,
            hw_power_brake_slowdown: mask & HW_POWER_BRAKE_SLOWDOWN != 0,
            display_clock_setting: mask & DISPLAY_CLOCK_SETTING != 0,
        }
    }

    /// Get all throttle reasons as (name, value) pairs for metrics
    pub fn as_pairs(&self) -> Vec<(&'static str, bool)> {
        vec![
            ("GpuIdle", self.gpu_idle),
            ("ApplicationsClocksSetting", self.applications_clocks_setting),
            ("SwPowerCap", self.sw_power_cap),
            ("HwSlowdown", self.hw_slowdown),
            ("SyncBoost", self.sync_boost),
            ("SwThermalSlowdown", self.sw_thermal_slowdown),
            ("HwThermalSlowdown", self.hw_thermal_slowdown),
            ("HwPowerBrakeSlowdown", self.hw_power_brake_slowdown),
            ("DisplayClockSetting", self.display_clock_setting),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_state_values() {
        // Dashboard expects: 0=unknown, 1=no error, 2=error, 3=vm, 5=hw_failure
        assert_eq!(GpuState::Ok.as_u8(), 1);             // no error
        assert_eq!(GpuState::Warning.as_u8(), 1);        // no error
        assert_eq!(GpuState::Error.as_u8(), 2);          // error
        assert_eq!(GpuState::VmPassthrough.as_u8(), 3);  // special state
        assert_eq!(GpuState::Unavailable.as_u8(), 0);    // unknown
        assert_eq!(GpuState::HardwareFailure.as_u8(), 5); // hardware failure
    }

    #[test]
    fn test_architecture_detection() {
        assert_eq!(
            GpuArchitecture::from_name("NVIDIA A100-SXM4-80GB"),
            GpuArchitecture::Datacenter
        );
        assert_eq!(
            GpuArchitecture::from_name("NVIDIA GeForce RTX 4090"),
            GpuArchitecture::Consumer
        );
        assert_eq!(
            GpuArchitecture::from_name("NVIDIA RTX A6000"),
            GpuArchitecture::Professional
        );
    }

    #[test]
    fn test_throttle_reasons() {
        let reasons = ThrottleReasons::from_bitmask(0b101); // GPU_IDLE | SW_POWER_CAP
        assert!(reasons.gpu_idle);
        assert!(!reasons.applications_clocks_setting);
        assert!(reasons.sw_power_cap);
    }
}
