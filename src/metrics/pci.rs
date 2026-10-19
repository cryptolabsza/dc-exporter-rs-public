//! PCIe Bus Scanner
//!
//! Detects all NVIDIA GPUs on the PCIe bus, even those passed through to VMs.
//! This uses sysfs instead of libpci for better reliability and no library dependency.
//!
//! SAFETY: This never blocks on GPU state - it only reads PCI configuration space
//! which is always accessible even when GPUs are in VM passthrough.

use std::fs;
use std::path::Path;
use tracing::{debug, trace};

/// Information about a GPU detected on the PCIe bus
#[derive(Debug, Clone)]
pub struct PciGpuInfo {
    /// PCI bus ID (e.g., "0000:01:00.0")
    pub pci_bus_id: String,
    
    /// Vendor ID (0x10DE for NVIDIA)
    pub vendor_id: u16,
    
    /// Device ID
    pub device_id: u16,
    
    /// Whether this GPU is also visible in NVML (not passed to VM)
    pub in_nvml: bool,
    
    /// BAR0 base address (for direct register access)
    pub bar0_addr: Option<u64>,
    
    /// Which driver is bound to this device (e.g., "nvidia", "vfio-pci", or None)
    pub bound_driver: Option<String>,
    
    /// Health information from /proc/driver/nvidia/gpus/ (if present)
    pub proc_health: Option<ProcGpuHealth>,
}

/// Health information parsed from /proc/driver/nvidia/gpus/<pci_bus_id>/information
/// A GPU that appears here but with broken fields indicates hardware failure
#[derive(Debug, Clone)]
pub struct ProcGpuHealth {
    /// GPU model name from /proc
    pub model: Option<String>,
    
    /// GPU UUID from /proc
    pub uuid: Option<String>,
    
    /// Video BIOS version - "??.??.??.??.??" indicates hardware failure
    pub video_bios: Option<String>,
    
    /// GPU Firmware version - "N/A" indicates hardware failure
    pub firmware: Option<String>,
    
    /// Whether this GPU appears healthy based on /proc info
    pub is_healthy: bool,
    
    /// Specific failure reason if unhealthy
    pub failure_reason: Option<String>,
}

/// Scan the PCIe bus for all NVIDIA GPUs
pub fn scan_nvidia_gpus() -> Vec<PciGpuInfo> {
    let mut gpus = Vec::new();
    let pci_devices = Path::new("/sys/bus/pci/devices");
    
    if !pci_devices.exists() {
        debug!("sysfs PCI devices not available");
        return gpus;
    }
    
    let entries = match fs::read_dir(pci_devices) {
        Ok(e) => e,
        Err(e) => {
            debug!("Failed to read PCI devices directory: {}", e);
            return gpus;
        }
    };
    
    for entry in entries.flatten() {
        let path = entry.path();
        
        // Read vendor ID
        let vendor_path = path.join("vendor");
        let vendor_id = match read_hex_file(&vendor_path) {
            Some(v) => v as u16,
            None => continue,
        };
        
        // Check for NVIDIA vendor ID (0x10DE)
        if vendor_id != 0x10DE {
            continue;
        }
        
        // Read device class to filter for GPUs (VGA/3D controller)
        let class_path = path.join("class");
        let class_id = match read_hex_file(&class_path) {
            Some(c) => c,
            None => continue,
        };
        
        // Class 0x030000 = VGA compatible controller
        // Class 0x030200 = 3D controller
        let class_prefix = (class_id >> 16) & 0xFF;
        if class_prefix != 0x03 {
            trace!("Skipping NVIDIA device {:?} - not a display controller (class=0x{:06X})", path, class_id);
            continue;
        }
        
        // Read device ID
        let device_path = path.join("device");
        let device_id = match read_hex_file(&device_path) {
            Some(d) => d as u16,
            None => continue,
        };
        
        // Get PCI bus ID from directory name
        let pci_bus_id = match entry.file_name().to_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        
        // Read BAR0 address from resource file
        let bar0_addr = read_bar0_address(&path);
        
        // Check which driver is bound (nvidia, vfio-pci, or none)
        let bound_driver = read_bound_driver(&path);
        
        // Check /proc/driver/nvidia/gpus/ for health info
        let proc_health = read_proc_gpu_health(&pci_bus_id);
        
        debug!(
            "Found NVIDIA GPU: {} (device_id=0x{:04X}, bar0={:?}, driver={:?}, proc_healthy={:?})",
            pci_bus_id, device_id, bar0_addr, bound_driver,
            proc_health.as_ref().map(|h| h.is_healthy)
        );
        
        gpus.push(PciGpuInfo {
            pci_bus_id,
            vendor_id,
            device_id,
            in_nvml: false, // Will be updated by collector
            bar0_addr,
            bound_driver,
            proc_health,
        });
    }
    
    // Sort by PCI bus ID for consistent ordering
    gpus.sort_by(|a, b| a.pci_bus_id.cmp(&b.pci_bus_id));
    
    debug!("Found {} NVIDIA GPUs on PCIe bus", gpus.len());
    gpus
}

/// Read a hexadecimal value from a sysfs file
fn read_hex_file(path: &Path) -> Option<u64> {
    let content = fs::read_to_string(path).ok()?;
    let content = content.trim();
    
    // Handle both "0x1234" and "1234" formats
    let content = content.strip_prefix("0x").unwrap_or(content);
    u64::from_str_radix(content, 16).ok()
}

/// Read BAR0 base address from PCI resource file
fn read_bar0_address(device_path: &Path) -> Option<u64> {
    // The resource file contains one line per BAR:
    // start end flags
    let resource_path = device_path.join("resource");
    let content = fs::read_to_string(resource_path).ok()?;
    
    // First line is BAR0
    let first_line = content.lines().next()?;
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    
    if parts.len() >= 2 {
        // Start address is first field
        let addr_str = parts[0].strip_prefix("0x").unwrap_or(parts[0]);
        let addr = u64::from_str_radix(addr_str, 16).ok()?;
        
        if addr != 0 {
            return Some(addr);
        }
    }
    
    None
}

/// Parse a PCI bus ID string into components
/// "0000:01:00.0" -> (domain=0, bus=1, device=0, function=0)
pub fn parse_pci_bus_id(pci_bus_id: &str) -> Option<(u32, u8, u8, u8)> {
    let parts: Vec<&str> = pci_bus_id.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    
    let domain = u32::from_str_radix(parts[0], 16).ok()?;
    let bus = u8::from_str_radix(parts[1], 16).ok()?;
    
    let dev_func: Vec<&str> = parts[2].split('.').collect();
    if dev_func.len() != 2 {
        return None;
    }
    
    let device = u8::from_str_radix(dev_func[0], 16).ok()?;
    let function = u8::from_str_radix(dev_func[1], 16).ok()?;
    
    Some((domain, bus, device, function))
}

/// Format PCI bus ID from components
pub fn format_pci_bus_id(domain: u32, bus: u8, device: u8, function: u8) -> String {
    format!("{:04x}:{:02x}:{:02x}.{}", domain, bus, device, function)
}

/// Read which driver is bound to a PCI device
/// Returns "nvidia", "vfio-pci", "pci-stub", or None
fn read_bound_driver(device_path: &Path) -> Option<String> {
    let driver_link = device_path.join("driver");
    let target = fs::read_link(&driver_link).ok()?;
    // Driver symlink target is like "../../../../bus/pci/drivers/nvidia"
    target.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
}

/// Drivers that indicate GPU is in VM passthrough
pub fn is_passthrough_driver(driver: &str) -> bool {
    matches!(driver, "vfio-pci" | "pci-stub")
}

/// Canonical PCI identity for assignment evidence and guest telemetry.
pub(super) fn canonical_pci_bus_id(value: &str) -> Option<String> {
    let parts: Vec<_> = value.split(':').collect();
    if parts.len() != 3 || !matches!(parts[0].len(), 4 | 8) || parts[1].len() != 2 {
        return None;
    }
    let function: Vec<_> = parts[2].split('.').collect();
    if function.len() != 2 || function[0].len() != 2 || function[1].len() != 1
        || !parts[0].bytes().chain(parts[1].bytes()).chain(function[0].bytes()).chain(function[1].bytes()).all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let (domain, bus, slot, function) = parse_pci_bus_id(value)?;
    (domain <= 0xffff && slot <= 0x1f && function <= 7)
        .then(|| format_pci_bus_id(domain, bus, slot, function))
}

// TRACEWEAVER: entrypoint=qemu_passthrough_from_process; req=VM-FIX-002; trace=TRACE-VM-002
fn qemu_passthrough_from_process(
    executable: Option<&Path>, argv: &[String],
) -> std::collections::HashSet<String> {
    let mut passthrough_pci = std::collections::HashSet::new();
    let Some(executable) = executable else { return passthrough_pci; };
    let Some(name) = executable.file_name().and_then(|name| name.to_str()) else { return passthrough_pci; };
    let system_bin = executable.parent() == Some(Path::new("/usr/bin"));
    let system_kvm = name == "qemu-kvm"
        && (system_bin || executable.parent() == Some(Path::new("/usr/libexec")));
    let system_qemu = system_bin && name.strip_prefix("qemu-system-").is_some_and(|arch| {
        !arch.is_empty() && arch.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    });
    if !system_kvm && !system_qemu { return passthrough_pci; }

    for (index, argument) in argv.iter().enumerate().skip(1) {
        let device = if argument == "-device" {
            argv.get(index + 1).map(String::as_str)
        } else {
            argument.strip_prefix("-device=")
        };
        let Some(device) = device else { continue; };
        let mut fields = device.split(',');
        if fields.next() != Some("vfio-pci") { continue; }
        let hosts: Vec<_> = fields.filter_map(|field| field.strip_prefix("host=")).collect();
        if hosts.len() != 1 { continue; }
        let host = if hosts[0].len() == 7 { format!("0000:{}", hosts[0]) } else { hosts[0].to_string() };
        if let Some(host) = canonical_pci_bus_id(&host) { passthrough_pci.insert(host); }
    }
    passthrough_pci
}

/// Scan running QEMU processes to find which PCI addresses are passed through
/// This works even without libvirt/virsh - it parses QEMU command lines directly.
/// Returns a set of PCI bus IDs (lowercase, e.g., "0000:c1:00.0") that are in passthrough.
pub fn scan_qemu_passthrough_pci() -> std::collections::HashSet<String> {
    let mut passthrough_pci = std::collections::HashSet::new();
    
    // Read /proc to find all QEMU processes
    let proc_dir = Path::new("/proc");
    let entries = match fs::read_dir(proc_dir) {
        Ok(e) => e,
        Err(_) => return passthrough_pci,
    };
    
    for entry in entries.flatten() {
        // Only look at numeric directories (PIDs)
        let name = entry.file_name();
        let _name_str = match name.to_str() {
            Some(s) if s.chars().all(|c| c.is_ascii_digit()) => s.to_string(),
            _ => continue,
        };
        
        // Read the command line
        let cmdline_path = entry.path().join("cmdline");
        let cmdline = match fs::read_to_string(&cmdline_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        
        let executable = fs::read_link(entry.path().join("exe")).ok();
        let argv: Vec<String> = cmdline.split('\0').map(str::to_string).collect();
        passthrough_pci.extend(qemu_passthrough_from_process(executable.as_deref(), &argv));

    }
    
    if !passthrough_pci.is_empty() {
        debug!("QEMU passthrough PCI addresses: {:?}", passthrough_pci);
    }
    
    passthrough_pci
}

/// Read GPU health information from /proc/driver/nvidia/gpus/<pci_bus_id>/information
/// This only exists for GPUs bound to the nvidia driver (not vfio-pci passthrough)
/// A GPU that appears here but with broken VBIOS/firmware indicates hardware failure
fn read_proc_gpu_health(pci_bus_id: &str) -> Option<ProcGpuHealth> {
    let info_path = format!("/proc/driver/nvidia/gpus/{}/information", pci_bus_id);
    let content = fs::read_to_string(&info_path).ok()?;
    
    let mut health = ProcGpuHealth {
        model: None,
        uuid: None,
        video_bios: None,
        firmware: None,
        is_healthy: true, // Assume healthy, check for failure markers
        failure_reason: None,
    };
    
    let mut failure_reasons = Vec::new();
    
    for line in content.lines() {
        let line = line.trim();
        
        if let Some(value) = line.strip_prefix("Model:") {
            health.model = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("GPU UUID:") {
            health.uuid = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Video BIOS:") {
            let bios = value.trim().to_string();
            // "??.??.??.??.??" indicates the GPU can't read its own VBIOS - hardware failure
            if bios.contains("??") {
                health.is_healthy = false;
                failure_reasons.push(format!("broken VBIOS: {}", bios));
            }
            health.video_bios = Some(bios);
        } else if let Some(value) = line.strip_prefix("GPU Firmware:") {
            let fw = value.trim().to_string();
            // "N/A" indicates the GPU firmware is not accessible - hardware failure
            if fw == "N/A" || fw.is_empty() {
                health.is_healthy = false;
                failure_reasons.push("firmware not accessible (N/A)".to_string());
            }
            health.firmware = Some(fw);
        }
    }
    
    if !failure_reasons.is_empty() {
        health.failure_reason = Some(failure_reasons.join("; "));
    }
    
    Some(health)
}

#[cfg(test)]
mod tests {
    use super::*;

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_definition_only_explicit_passthrough_bindings_prove_reservation() {
        for binding in ["vfio-pci", "pci-stub"] {
            assert!(is_passthrough_driver(binding), "{binding}");
        }
        for binding in ["nvidia", "nouveau", "", "vfio-pci-fake", "pci_stub", "qemu"] {
            assert!(!is_passthrough_driver(binding), "{binding}");
        }
    }

    #[test]
    fn test_parse_pci_bus_id() {
        let result = parse_pci_bus_id("0000:01:00.0");
        assert_eq!(result, Some((0, 1, 0, 0)));

        let result = parse_pci_bus_id("0000:c1:00.0");
        assert_eq!(result, Some((0, 0xc1, 0, 0)));
    }

    #[test]
    fn test_format_pci_bus_id() {
        let result = format_pci_bus_id(0, 1, 0, 0);
        assert_eq!(result, "0000:01:00.0");
    }
    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_definition_qemu_requires_verified_executable_and_vfio_device_option() {
        let verified = Some(Path::new("/usr/bin/qemu-system-x86_64"));
        for (exe, argv, expected) in [
            (verified, vec!["qemu-system-x86_64", "-device", "vfio-pci,host=0000:C1:00.0"], vec!["0000:c1:00.0"]),
            (verified, vec!["qemu-system-x86_64", "-device", "vfio-pci,host=c1:00.0"], vec!["0000:c1:00.0"]),
            (None, vec!["qemu-system-x86_64", "-device", "vfio-pci,host=0000:c1:00.0"], vec![]),
            (Some(Path::new("/usr/bin/python3")), vec!["qemu-system-x86_64", "-device", "vfio-pci,host=0000:c1:00.0"], vec![]),
            (Some(Path::new("/usr/bin/echo")), vec!["echo", "qemu", "host=0000:c1:00.0"], vec![]),
            (verified, vec!["qemu-system-x86_64", "-name", "qemu,host=0000:c1:00.0"], vec![]),
            (verified, vec!["qemu-system-x86_64", "-device", "e1000,host=0000:c1:00.0"], vec![]),
            (verified, vec!["qemu-system-x86_64", "-append", "vfio-pci,host=0000:c1:00.0"], vec![]),
            (verified, vec!["qemu-system-x86_64", "-device", "vfio-pci,host=0000:zz:00.0"], vec![]),
            (verified, vec!["qemu-system-x86_64", "-device", "vfio-pci,host=0000:c1:20.0"], vec![]),
            (verified, vec!["qemu-system-x86_64", "-device", "vfio-pci,host=0000:c1:00.8"], vec![]),
        ] {
            let argv: Vec<String> = argv.into_iter().map(str::to_string).collect();
            let actual = qemu_passthrough_from_process(exe, &argv);
            assert_eq!(actual, expected.into_iter().map(str::to_string).collect(), "{exe:?} {argv:?}");
        }
    }

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_definition_qemu_writable_path_and_deleted_executable_are_not_proof() {
        let argv = vec!["qemu-system-x86_64".into(), "-device".into(), "vfio-pci,host=0000:c1:00.0".into()];
        for executable in ["/tmp/qemu-system-x86_64", "/var/tmp/qemu-kvm", "/usr/bin/qemu-system-x86_64 (deleted)"] {
            assert!(qemu_passthrough_from_process(Some(Path::new(executable)), &argv).is_empty(), "{executable}");
        }
        let argv = vec!["qemu-kvm".into(), "-device=vfio-pci,host=0001:C1:00.0".into()];
        assert_eq!(qemu_passthrough_from_process(Some(Path::new("/usr/libexec/qemu-kvm")), &argv),
            std::collections::HashSet::from(["0001:c1:00.0".into()]));
    }

}
