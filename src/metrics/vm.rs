//! VM GPU Metrics Collection
//!
//! Queries GPU metrics from inside VMs via QEMU guest agent.
//! This allows collecting metrics even when GPUs are passed through to VMs.
//!
//! Method:
//! 1. Use `virsh list` to find running VMs
//! 2. Use `virsh dumpxml` to find which GPUs are passed to each VM
//! 3. Use `virsh qemu-agent-command` to execute nvidia-smi inside the VM
//! 4. Use guest agent to read VRAM/hotspot temps via sysfs inside VM
//!
//! Requirements:
//! - libvirt/virsh installed on host
//! - QEMU guest agent running inside VMs
//! - nvidia drivers installed inside VMs
//!
//! SECURITY:
//! - VM names are validated to prevent command injection

use std::collections::HashMap;
use std::process::Command;
use base64::Engine;
use tracing::{debug, trace, warn};

use crate::state::{GpuState, ThrottleReasons};
use super::{GpuMetrics, MetricSource};

/// Validate VM name to prevent command injection
/// Only allows alphanumeric, dash, underscore, and dot characters
fn is_valid_vm_name(name: &str) -> bool {
    !name.is_empty() 
        && name.len() <= 253  // Max DNS label length
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !name.starts_with('-')
        && !name.starts_with('.')
}

/// Script to read VRAM/Hotspot temps inside VM via sysfs
/// Uses mmap on /sys/bus/pci/devices/*/resource0
const VM_TEMP_READER_PYTHON: &str = r#"
import os,mmap,struct,json,glob
r=[]
for p in sorted(glob.glob("/sys/bus/pci/devices/*/vendor")):
 try:
  if open(p).read().strip()!="0x10de":continue
  d=os.path.dirname(p)
  f=os.open(d+"/resource0",os.O_RDONLY|os.O_SYNC)
  m=mmap.mmap(f,0,mmap.MAP_SHARED,mmap.PROT_READ)
  m.seek(0xE2A8);v=(struct.unpack("<I",m.read(4))[0]&0xfff)//32
  m.seek(0x2046c);h=(struct.unpack("<I",m.read(4))[0]>>8)&0x7f
  m.close();os.close(f)
  r.append({"p":os.path.basename(d),"v":v,"h":h})
 except:pass
print(json.dumps(r))
"#;

/// Shell fallback for reading temps (works on minimal distros)
const VM_TEMP_READER_SHELL: &str = r#"
for d in /sys/bus/pci/devices/*;do
 [ -f "$d/vendor" ]||continue
 read v<"$d/vendor";[ "$v" = "0x10de" ]||continue
 vr=$(dd if="$d/resource0" bs=4 skip=14634 count=1 2>/dev/null|od -An -tu4|tr -d ' ')
 vt=$(((vr&4095)/32))
 hr=$(dd if="$d/resource0" bs=4 skip=33051 count=1 2>/dev/null|od -An -tu4|tr -d ' ')
 ht=$(((hr>>8)&127))
 echo "${d##*/}:$vt:$ht"
done
"#;

/// VM metrics collector
pub struct VmMetricsCollector;

impl VmMetricsCollector {
    /// Check if virsh is available
    pub fn is_available() -> bool {
        Command::new("which")
            .arg("virsh")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get list of running VMs
    pub fn get_running_vms() -> Vec<String> {
        let output = match Command::new("virsh")
            .args(["list", "--name"])
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                debug!("Failed to run virsh list: {}", e);
                return Vec::new();
            }
        };

        if !output.status.success() {
            return Vec::new();
        }

        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
            .collect()
    }

    /// Get GPU passthrough configuration for a VM
    /// Returns list of host PCI addresses passed to this VM
    pub fn get_vm_gpu_passthrough(vm_name: &str) -> Vec<String> {
        // Security: validate VM name to prevent command injection
        if !is_valid_vm_name(vm_name) {
            warn!("Invalid VM name rejected: {:?}", vm_name);
            return Vec::new();
        }
        
        let output = match Command::new("virsh")
            .args(["dumpxml", vm_name])
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                debug!("Failed to get VM XML for {}: {}", vm_name, e);
                return Vec::new();
            }
        };

        if !output.status.success() {
            return Vec::new();
        }

        let xml = String::from_utf8_lossy(&output.stdout);
        parse_gpu_passthrough_xml(&xml)
    }

    /// Execute command inside VM via QEMU guest agent
    /// 
    /// NOTE: This function uses blocking operations (subprocess + sleep) intentionally.
    /// It runs in a blocking context via tokio::task::spawn_blocking, so blocking
    /// sleep is appropriate here. The virsh commands are inherently blocking anyway.
    pub fn guest_exec(vm_name: &str, command: &str, args: &[&str]) -> Option<(String, i32)> {
        // Security: validate VM name to prevent command injection
        if !is_valid_vm_name(vm_name) {
            warn!("Invalid VM name rejected in guest_exec: {:?}", vm_name);
            return None;
        }
        
        // Build guest-exec JSON command
        let args_json: Vec<String> = args.iter().map(|a| format!("\"{}\"", a)).collect();
        let exec_cmd = format!(
            r#"{{"execute":"guest-exec","arguments":{{"path":"{}","arg":[{}],"capture-output":true}}}}"#,
            command,
            args_json.join(",")
        );

        // Execute via virsh
        let output = Command::new("virsh")
            .args(["qemu-agent-command", vm_name, &exec_cmd])
            .output()
            .ok()?;

        if !output.status.success() {
            debug!("guest-exec failed for {}", vm_name);
            return None;
        }

        // Parse PID from response
        let response = String::from_utf8_lossy(&output.stdout);
        let pid: i64 = serde_json::from_str::<serde_json::Value>(&response)
            .ok()?
            .get("return")?
            .get("pid")?
            .as_i64()?;

        // Wait a bit for command to complete
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Get result
        let status_cmd = format!(
            r#"{{"execute":"guest-exec-status","arguments":{{"pid":{}}}}}"#,
            pid
        );

        let output = Command::new("virsh")
            .args(["qemu-agent-command", vm_name, &status_cmd])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let response = String::from_utf8_lossy(&output.stdout);
        let result: serde_json::Value = serde_json::from_str(&response).ok()?;
        let ret = result.get("return")?;

        // Check if exited
        if !ret.get("exited")?.as_bool()? {
            // Wait more and retry
            std::thread::sleep(std::time::Duration::from_secs(1));
            let output = Command::new("virsh")
                .args(["qemu-agent-command", vm_name, &status_cmd])
                .output()
                .ok()?;
            let response = String::from_utf8_lossy(&output.stdout);
            let result: serde_json::Value = serde_json::from_str(&response).ok()?;
            let ret = result.get("return")?;
            
            if !ret.get("exited")?.as_bool().unwrap_or(false) {
                return None;
            }
        }

        // Decode base64 output
        let stdout_b64 = ret.get("out-data")?.as_str()?;
        let stdout = String::from_utf8(
            base64::engine::general_purpose::STANDARD.decode(stdout_b64).ok()?
        ).ok()?;

        let exit_code = ret.get("exitcode")?.as_i64().unwrap_or(-1) as i32;

        Some((stdout, exit_code))
    }

    /// Get VRAM/Hotspot temps from inside VM via sysfs
    pub fn get_vm_gpu_temps(vm_name: &str) -> HashMap<String, (u32, u32)> {
        let mut temps = HashMap::new();

        // Try Python first
        if let Some((stdout, 0)) = Self::guest_exec(
            vm_name,
            "/usr/bin/python3",
            &["-c", VM_TEMP_READER_PYTHON],
        ) {
            if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(&stdout.trim()) {
                for r in results {
                    if let (Some(pci), Some(vram), Some(hotspot)) = (
                        r.get("p").and_then(|v| v.as_str()),
                        r.get("v").and_then(|v| v.as_u64()),
                        r.get("h").and_then(|v| v.as_u64()),
                    ) {
                        temps.insert(pci.to_string(), (vram as u32, hotspot as u32));
                    }
                }
                if !temps.is_empty() {
                    return temps;
                }
            }
        }

        // Fallback to shell
        if let Some((stdout, 0)) = Self::guest_exec(vm_name, "/bin/sh", &["-c", VM_TEMP_READER_SHELL])
        {
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 3 {
                    if let (Ok(vram), Ok(hotspot)) =
                        (parts[1].parse::<u32>(), parts[2].parse::<u32>())
                    {
                        temps.insert(parts[0].to_string(), (vram, hotspot));
                    }
                }
            }
        }

        temps
    }

    /// Get GPU metrics from inside VM via nvidia-smi
    pub fn get_vm_gpu_metrics(vm_name: &str, host_gpu_indices: &[u32]) -> Vec<GpuMetrics> {
        let mut gpus = Vec::new();

        // Run nvidia-smi inside VM
        let nvidia_smi_args = [
            "--query-gpu=index,name,uuid,pci.bus_id,temperature.gpu,fan.speed,power.draw,utilization.gpu,utilization.memory,memory.used,memory.total,clocks.sm,clocks.mem,driver_version",
            "--format=csv,noheader,nounits"
        ];

        let (stdout, exit_code) = match Self::guest_exec(vm_name, "/usr/bin/nvidia-smi", &nvidia_smi_args) {
            Some(r) => r,
            None => {
                debug!("Failed to run nvidia-smi in VM {}", vm_name);
                return gpus;
            }
        };

        if exit_code != 0 {
            debug!("nvidia-smi failed in VM {} with exit code {}", vm_name, exit_code);
            return gpus;
        }

        // Get VRAM/Hotspot temps via sysfs
        let vm_temps = Self::get_vm_gpu_temps(vm_name);

        // Parse nvidia-smi output
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
            if parts.len() < 14 {
                continue;
            }

            let vm_gpu_idx: u32 = match parts[0].parse() {
                Ok(i) => i,
                Err(_) => continue,
            };

            // Map VM GPU index to host GPU index
            let host_gpu_idx = host_gpu_indices
                .get(vm_gpu_idx as usize)
                .copied()
                .unwrap_or(vm_gpu_idx);

            // Get VM's PCI bus ID and look up temps
            let vm_pci_bus = parts[3].replace("00000000:", "0000:");
            let (vram_temp, hotspot_temp) = vm_temps
                .iter()
                .find(|(k, _)| vm_pci_bus.contains(k.as_str()))
                .map(|(_, v)| *v)
                .unwrap_or((0, 0));

            let mut gpu = GpuMetrics {
                index: host_gpu_idx,
                name: parts[1].to_string(),
                uuid: parts[2].to_string(),
                pci_bus_id: vm_pci_bus,
                driver_version: parts.get(13).unwrap_or(&"N/A").to_string(),
                state: GpuState::Ok,
                source: MetricSource::Vm,
                
                gpu_temp: parse_metric(parts[4]),
                fan_speed: parse_metric(parts[5]),
                power_usage: parts[6].parse::<f64>().ok(),
                gpu_util: parse_metric(parts[7]),
                mem_util: parse_metric(parts[8]),
                fb_used: parts[9].parse::<u64>().ok(),
                fb_free: parts[10].parse::<u64>().ok().and_then(|total| {
                    parts[9].parse::<u64>().ok().map(|used| total.saturating_sub(used))
                }),
                sm_clock: parse_metric(parts[11]),
                mem_clock: parse_metric(parts[12]),
                
                vram_temp: if vram_temp > 0 { Some(vram_temp) } else { None },
                hotspot_temp: if hotspot_temp > 0 { Some(hotspot_temp) } else { None },
                vram_temp_approximated: false,
                hotspot_temp_approximated: false,
                
                throttle_reasons: ThrottleReasons::default(),
                aer_errors: 0,
                ..Default::default()
            };

            // Mark total memory
            if let Ok(total) = parts[10].parse::<u64>() {
                if let Some(used) = gpu.fb_used {
                    gpu.fb_free = Some(total.saturating_sub(used));
                }
            }

            gpus.push(gpu);
        }

        gpus
    }

    /// Get ALL PCI bus IDs that are passed through to running VMs via virsh dumpxml.
    /// This works even without guest agent - it only needs libvirt/virsh on the host.
    /// Returns a set of PCI bus IDs (e.g., "0000:c1:00.0") that are confirmed in passthrough.
    pub fn get_all_passthrough_pci_ids() -> Vec<String> {
        if !Self::is_available() {
            return Vec::new();
        }

        let vms = Self::get_running_vms();
        let mut all_pci = Vec::new();

        for vm_name in &vms {
            let passthrough_pci = Self::get_vm_gpu_passthrough(vm_name);
            for pci_id in passthrough_pci {
                debug!("VM {} has GPU passthrough: {}", vm_name, pci_id);
                all_pci.push(pci_id);
            }
        }

        all_pci
    }

    /// Collect metrics from all running VMs
    pub fn collect_all(host_gpu_map: &HashMap<String, u32>) -> Vec<GpuMetrics> {
        if !Self::is_available() {
            debug!("virsh not available - VM metrics disabled");
            return Vec::new();
        }

        let vms = Self::get_running_vms();
        if vms.is_empty() {
            debug!("No running VMs found");
            return Vec::new();
        }

        let mut all_metrics = Vec::new();

        for vm_name in vms {
            // Get which host GPUs are passed to this VM
            let passthrough_pci = Self::get_vm_gpu_passthrough(&vm_name);
            if passthrough_pci.is_empty() {
                trace!("VM {} has no GPU passthrough", vm_name);
                continue;
            }

            // Map host PCI addresses to GPU indices
            let host_indices: Vec<u32> = passthrough_pci
                .iter()
                .filter_map(|pci| host_gpu_map.get(pci).copied())
                .collect();

            debug!(
                "VM {} has {} passthrough GPUs: {:?}",
                vm_name,
                host_indices.len(),
                passthrough_pci
            );

            // Collect metrics from inside the VM
            let vm_metrics = Self::get_vm_gpu_metrics(&vm_name, &host_indices);
            all_metrics.extend(vm_metrics);
        }

        all_metrics
    }
}

/// Parse GPU passthrough from VM XML
fn parse_gpu_passthrough_xml(xml: &str) -> Vec<String> {
    let mut gpu_slots = Vec::new();

    // Simple XML parsing - look for hostdev type="pci" entries
    // Format: <address domain="0x0000" bus="0x01" slot="0x00" function="0x0"/>
    
    let mut in_hostdev = false;
    let mut is_pci = false;

    for line in xml.lines() {
        let line = line.trim();
        
        if line.contains("<hostdev") && line.contains("type='pci'") {
            in_hostdev = true;
            is_pci = true;
        } else if line.contains("</hostdev>") {
            in_hostdev = false;
            is_pci = false;
        }
        
        if in_hostdev && is_pci && line.contains("<address") && line.contains("domain=") {
            // Extract domain, bus, slot, function
            if let Some(addr) = parse_pci_address(line) {
                // Only add function 0 (the GPU itself, not audio)
                if addr.ends_with(".0") {
                    gpu_slots.push(addr);
                }
            }
        }
    }

    gpu_slots
}

/// Parse PCI address from XML line
fn parse_pci_address(line: &str) -> Option<String> {
    let get_attr = |name: &str| -> Option<String> {
        let pattern = format!("{}='", name);
        let start = line.find(&pattern)? + pattern.len();
        let end = line[start..].find('\'')?;
        Some(line[start..start + end].to_string())
    };

    let domain = get_attr("domain")?.strip_prefix("0x").unwrap_or("0000").to_string();
    let bus = get_attr("bus")?.strip_prefix("0x").unwrap_or("00").to_string();
    let slot = get_attr("slot")?.strip_prefix("0x").unwrap_or("00").to_string();
    let function = get_attr("function")?.strip_prefix("0x").unwrap_or("0").to_string();

    Some(format!("{}:{}:{}.{}", domain, bus, slot, function))
}

/// Parse a metric value that might be "N/A" or "[N/A]"
fn parse_metric(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() || s == "N/A" || s == "[N/A]" || s == "[Not Supported]" {
        return None;
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pci_address() {
        let line = r#"<address domain='0x0000' bus='0x01' slot='0x00' function='0x0'/>"#;
        assert_eq!(parse_pci_address(line), Some("0000:01:00.0".to_string()));
    }

    #[test]
    fn test_parse_metric() {
        assert_eq!(parse_metric("65"), Some(65));
        assert_eq!(parse_metric("N/A"), None);
        assert_eq!(parse_metric("[N/A]"), None);
        assert_eq!(parse_metric("  42  "), Some(42));
    }

    #[test]
    fn test_valid_vm_names() {
        // Valid names
        assert!(is_valid_vm_name("my-vm"));
        assert!(is_valid_vm_name("vm_001"));
        assert!(is_valid_vm_name("test.vm.local"));
        assert!(is_valid_vm_name("Ubuntu20-04"));
        assert!(is_valid_vm_name("a"));
        
        // Invalid names - command injection attempts
        assert!(!is_valid_vm_name(""));
        assert!(!is_valid_vm_name("vm; rm -rf /"));
        assert!(!is_valid_vm_name("vm`whoami`"));
        assert!(!is_valid_vm_name("vm$(cat /etc/passwd)"));
        assert!(!is_valid_vm_name("vm | cat /etc/shadow"));
        assert!(!is_valid_vm_name("-vm"));  // starts with dash
        assert!(!is_valid_vm_name(".vm"));  // starts with dot
        assert!(!is_valid_vm_name("vm\nmalicious"));  // newline
        assert!(!is_valid_vm_name("vm\x00null"));  // null byte
    }
}
