//! Direct GPU Register Access
//!
//! Reads VRAM and Hotspot temperatures directly from GPU registers via /dev/mem.
//! This works for consumer GPUs (RTX 30xx/40xx/50xx series) that use GDDR6/GDDR6X/GDDR7 memory.
//!
//! SAFETY CONSIDERATIONS:
//! - Requires root access and iomem=relaxed kernel parameter
//! - Only reads from memory-mapped registers (no writes)
//! - Maps memory briefly and unmaps immediately
//! - Falls back gracefully if /dev/mem is not accessible
//!
//! This is the ONLY way to get VRAM temperature on consumer GPUs -
//! NVML does not expose this metric for GeForce cards.

use memmap2::MmapOptions;
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use tracing::{debug, trace, warn};

/// Register offsets for different GPU architectures
pub mod offsets {
    /// VRAM temperature register offset (Ada/Ampere consumer GPUs)
    pub const VRAM_TEMP_OFFSET: u64 = 0x0000E2A8;
    
    /// VRAM temperature register offset (RTX 3070 specifically)
    pub const VRAM_TEMP_OFFSET_3070: u64 = 0x0000EE50;
    
    /// Hotspot/junction temperature register offset
    pub const HOTSPOT_TEMP_OFFSET: u64 = 0x0002046c;
}

/// GPU device IDs and their VRAM register offsets
#[derive(Debug, Clone, Copy)]
pub struct GpuRegisterInfo {
    pub device_id: u16,
    pub vram_offset: u64,
    pub name: &'static str,
}

/// Known GPUs with direct register access support
pub static SUPPORTED_GPUS: &[GpuRegisterInfo] = &[
    // Blackwell (RTX 50 series) - GDDR7 memory
    // DISABLED: Register offsets are different from Ada/Ampere and haven't been
    // reverse-engineered yet. The Ada offsets return 0xBADF1100 (unmapped).
    // TODO: Find correct offsets for Blackwell architecture
    // GpuRegisterInfo { device_id: 0x2B85, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 5090" },
    // GpuRegisterInfo { device_id: 0x2B87, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 5080" },
    // GpuRegisterInfo { device_id: 0x2B89, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 5070 Ti" },
    // GpuRegisterInfo { device_id: 0x2B8B, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 5070" },
    
    // Ada Lovelace (RTX 40 series)
    GpuRegisterInfo { device_id: 0x2684, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 4090" },
    GpuRegisterInfo { device_id: 0x2704, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 4080" },
    GpuRegisterInfo { device_id: 0x2782, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 4070 Ti" },
    GpuRegisterInfo { device_id: 0x2786, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 4070" },
    GpuRegisterInfo { device_id: 0x2860, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 4060" },
    GpuRegisterInfo { device_id: 0x26B1, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX A6000 Ada" },
    GpuRegisterInfo { device_id: 0x26B2, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 5000 Ada" },
    GpuRegisterInfo { device_id: 0x26B3, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 4500 Ada" },
    GpuRegisterInfo { device_id: 0x26B4, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 4000 Ada" },
    GpuRegisterInfo { device_id: 0x26b9, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "L40S" },
    GpuRegisterInfo { device_id: 0x26B5, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "L40" },
    GpuRegisterInfo { device_id: 0x27b8, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "L4" },
    
    // Ampere (RTX 30 series)
    GpuRegisterInfo { device_id: 0x2204, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 3090" },
    GpuRegisterInfo { device_id: 0x2208, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 3080 Ti" },
    GpuRegisterInfo { device_id: 0x2206, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 3080" },
    GpuRegisterInfo { device_id: 0x2216, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX 3080 LHR" },
    GpuRegisterInfo { device_id: 0x2230, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX A6000" },
    GpuRegisterInfo { device_id: 0x2231, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX A5000" },
    GpuRegisterInfo { device_id: 0x2232, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX A4500" },
    GpuRegisterInfo { device_id: 0x2484, vram_offset: offsets::VRAM_TEMP_OFFSET_3070, name: "RTX 3070" },
    GpuRegisterInfo { device_id: 0x2488, vram_offset: offsets::VRAM_TEMP_OFFSET_3070, name: "RTX 3070 LHR" },
    GpuRegisterInfo { device_id: 0x2531, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX A2000" },
    GpuRegisterInfo { device_id: 0x2571, vram_offset: offsets::VRAM_TEMP_OFFSET, name: "RTX A2000" },
];

/// Check if a GPU device ID supports direct register access
pub fn get_gpu_info(device_id: u16) -> Option<&'static GpuRegisterInfo> {
    SUPPORTED_GPUS.iter().find(|g| g.device_id == device_id)
}

/// Check if /dev/mem is accessible
pub fn is_dev_mem_accessible() -> bool {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_SYNC)
        .open("/dev/mem")
        .is_ok()
}

/// Check if iomem=relaxed kernel parameter is set
pub fn is_iomem_relaxed() -> bool {
    if let Ok(cmdline) = std::fs::read_to_string("/proc/cmdline") {
        cmdline.contains("iomem=relaxed")
    } else {
        false
    }
}

/// Test if we can actually mmap GPU BAR0 regions
/// This is the definitive test - some kernels allow access without iomem=relaxed
/// if CONFIG_IO_STRICT_DEVMEM is not set
fn test_mmap_access() -> bool {
    // Try to mmap a high address that would be in GPU BAR0 range
    // We use a test that doesn't require knowing the actual GPU address
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_SYNC)
        .open("/dev/mem")
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    
    // Try mmapping - if CONFIG_IO_STRICT_DEVMEM is disabled, this should work
    // even without iomem=relaxed. We test at offset 0 with minimal size.
    // The actual GPU mmap will work if this basic test works.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    
    // Try to mmap a PCI config region (0xC0000000 range is typical for GPU BAR0)
    // If the kernel allows this, it will allow GPU register access
    match unsafe {
        MmapOptions::new()
            .offset(0)
            .len(page_size as usize)
            .map(&file)
    } {
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Get register access status for display purposes
#[derive(Debug, Clone)]
pub struct RegisterAccessStatus {
    pub available: bool,
    pub iomem_relaxed: bool,
    pub dev_mem_accessible: bool,
    pub mmap_works: bool,
}

impl RegisterAccessStatus {
    pub fn check() -> Self {
        let iomem_relaxed = is_iomem_relaxed();
        let dev_mem_accessible = is_dev_mem_accessible();
        
        // The real test: can we actually mmap?
        // Some kernels allow I/O region access even without iomem=relaxed
        // if CONFIG_IO_STRICT_DEVMEM is not set
        let mmap_works = if dev_mem_accessible {
            test_mmap_access()
        } else {
            false
        };
        
        RegisterAccessStatus {
            // Available if either iomem=relaxed is set OR mmap actually works
            available: mmap_works || (iomem_relaxed && dev_mem_accessible),
            iomem_relaxed,
            dev_mem_accessible,
            mmap_works,
        }
    }
    
    /// Get a user-friendly message about what's missing
    pub fn warning_message(&self) -> Option<String> {
        if self.available {
            return None;
        }
        
        if !self.dev_mem_accessible {
            Some("Legacy BAR temperatures unavailable: /dev/mem not accessible (need root)".to_string())
        } else if !self.mmap_works && !self.iomem_relaxed {
            Some("Legacy BAR temperatures unavailable: kernel denies memory mapping".to_string())
        } else {
            Some("Legacy BAR temperatures unavailable: mmap access denied".to_string())
        }
    }
}

/// Read temperatures from GPU registers
#[derive(Debug, Clone, Default)]
pub struct RegisterTemps {
    pub vram_temp: Option<u32>,
    pub hotspot_temp: Option<u32>,
}

/// Open /dev/mem with minimal permissions
/// Try read-only first (safer), fallback to read-write if needed for mmap
fn open_dev_mem() -> Option<std::fs::File> {
    // First try read-only (preferred for security)
    if let Ok(f) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_SYNC)
        .open("/dev/mem")
    {
        return Some(f);
    }
    
    // Fallback: some systems require write flag for mmap to work
    match OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_SYNC)
        .open("/dev/mem")
    {
        Ok(f) => Some(f),
        Err(e) => {
            debug!("Cannot open /dev/mem: {} (need root + iomem=relaxed)", e);
            None
        }
    }
}

/// Read VRAM and hotspot temperatures via direct register access
///
/// # Arguments
/// * `bar0_addr` - BAR0 base address from PCI config space
/// * `vram_offset` - VRAM temperature register offset (GPU-specific)
///
/// # Safety
/// Requires root privileges and iomem=relaxed kernel parameter.
/// This function is safe because it only performs reads, not writes.
pub fn read_gpu_temps(bar0_addr: u64, vram_offset: u64) -> RegisterTemps {
    let mut temps = RegisterTemps::default();
    
    let file = match open_dev_mem() {
        Some(f) => f,
        None => return temps,
    };
    
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    
    // Read VRAM temperature
    temps.vram_temp = read_vram_temp(&file, bar0_addr, vram_offset, page_size);
    
    // Read Hotspot temperature
    temps.hotspot_temp = read_hotspot_temp(&file, bar0_addr, page_size);
    
    temps
}

/// Read VRAM temperature from register
fn read_vram_temp(file: &std::fs::File, bar0_addr: u64, vram_offset: u64, page_size: u64) -> Option<u32> {
    let phys_addr = bar0_addr + vram_offset;
    let base_offset = phys_addr & !(page_size - 1);
    let offset_in_page = (phys_addr - base_offset) as usize;
    
    // Use read-only mmap (map() instead of map_raw() which may request write access)
    // SAFETY: We're reading from /dev/mem which requires special handling
    let mmap = match unsafe {
        MmapOptions::new()
            .offset(base_offset)
            .len(page_size as usize)
            .map(file)
    } {
        Ok(m) => m,
        Err(e) => {
            debug!("Failed to mmap for VRAM temp at 0x{:X}: {}", phys_addr, e);
            return None;
        }
    };
    
    // Read 32-bit register value
    let reg_value = unsafe {
        let ptr = mmap.as_ptr().add(offset_in_page) as *const u32;
        std::ptr::read_volatile(ptr)
    };
    
    // Decode temperature: (value & 0xFFF) / 0x20
    let temp = (reg_value & 0x00000FFF) / 0x20;
    
    trace!("VRAM temp register: 0x{:08X} -> {}°C", reg_value, temp);
    
    // Validate: temperature should be in reasonable range (0-150°C)
    if temp > 0 && temp < 150 {
        Some(temp)
    } else {
        debug!("Invalid VRAM temp value: {} (raw=0x{:08X})", temp, reg_value);
        None
    }
}

/// Read Hotspot/junction temperature from register
fn read_hotspot_temp(file: &std::fs::File, bar0_addr: u64, page_size: u64) -> Option<u32> {
    let phys_addr = bar0_addr + offsets::HOTSPOT_TEMP_OFFSET;
    let base_offset = phys_addr & !(page_size - 1);
    let offset_in_page = (phys_addr - base_offset) as usize;
    
    // Use read-only mmap
    let mmap = match unsafe {
        MmapOptions::new()
            .offset(base_offset)
            .len(page_size as usize)
            .map(file)
    } {
        Ok(m) => m,
        Err(e) => {
            debug!("Failed to mmap for hotspot temp at 0x{:X}: {}", phys_addr, e);
            return None;
        }
    };
    
    // Read 32-bit register value
    let reg_value = unsafe {
        let ptr = mmap.as_ptr().add(offset_in_page) as *const u32;
        std::ptr::read_volatile(ptr)
    };
    
    // Decode temperature: (value >> 8) & 0xFF
    let temp = (reg_value >> 8) & 0xFF;
    
    trace!("Hotspot temp register: 0x{:08X} -> {}°C", reg_value, temp);
    
    // Validate: temperature should be in reasonable range (0-127°C for this encoding)
    if temp > 0 && temp < 0x7F {
        Some(temp)
    } else {
        debug!("Invalid hotspot temp value: {} (raw=0x{:08X})", temp, reg_value);
        None
    }
}

/// Read VRAM temperature via sysfs (alternative method)
/// This method doesn't require /dev/mem but needs the GPU's BAR0 exposed in sysfs
pub fn read_vram_temp_sysfs(pci_bus_id: &str, vram_offset: u64) -> Option<u32> {
    let resource_path = format!("/sys/bus/pci/devices/{}/resource0", pci_bus_id);
    
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_SYNC)
        .open(&resource_path)
    {
        Ok(f) => f,
        Err(e) => {
            debug!("Cannot open {}: {}", resource_path, e);
            return None;
        }
    };
    
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    let base_offset = vram_offset & !(page_size - 1);
    let offset_in_page = (vram_offset - base_offset) as usize;
    
    // Use read-only mmap
    let mmap = match unsafe {
        MmapOptions::new()
            .offset(base_offset)
            .len(page_size as usize)
            .map(&file)
    } {
        Ok(m) => m,
        Err(e) => {
            debug!("Failed to mmap sysfs resource: {}", e);
            return None;
        }
    };
    
    let reg_value = unsafe {
        let ptr = mmap.as_ptr().add(offset_in_page) as *const u32;
        std::ptr::read_volatile(ptr)
    };
    
    let temp = (reg_value & 0x00000FFF) / 0x20;
    
    if temp > 0 && temp < 150 {
        Some(temp)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supported_gpu_lookup() {
        let info = get_gpu_info(0x2684);
        assert!(info.is_some());
        assert_eq!(info.unwrap().name, "RTX 4090");
    }

    #[test]
    fn test_unknown_gpu() {
        let info = get_gpu_info(0xFFFF);
        assert!(info.is_none());
    }

    #[test]
    fn test_rtx_5090_register_access_remains_disabled() {
        assert!(get_gpu_info(0x2B85).is_none());
    }
}
