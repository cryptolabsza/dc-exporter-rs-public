//! VM GPU Metrics Collection
//!
//! Queries GPU metrics from inside VMs via QEMU guest agent.
//! This allows collecting metrics even when GPUs are passed through to VMs.
//!
//! Method:
//! 1. Use `virsh list` to find running VMs
//! 2. Use `virsh dumpxml` to find which GPUs are passed to each VM
//! 3. Use `virsh qemu-agent-command` to execute nvidia-smi inside the VM
//! Guest BAR temperatures remain disabled until a guest profile is qualified.
//!
//! Requirements:
//! - libvirt/virsh installed on host
//! - QEMU guest agent running inside VMs
//! - nvidia drivers installed inside VMs
//!
//! SECURITY:
//! - VM names are validated to prevent command injection

use std::collections::HashMap;
use quick_xml::{events::{BytesStart, Event}, Reader, XmlVersion};
use super::pci::canonical_pci_bus_id;

use std::process::Command;
use base64::Engine;
use tracing::{debug, warn};

use crate::state::{GpuState, ThrottleReasons};
use super::{GpuMetrics, MetricSource};

type VmGpuMappings = HashMap<String, (String, u32, u16)>;
type VmGpuDomain = (String, Vec<String>, VmGpuMappings);

/// Validate VM name to prevent command injection
/// Only allows alphanumeric, dash, underscore, and dot characters
fn is_valid_vm_name(name: &str) -> bool {
    !name.is_empty() 
        && name.len() <= 253  // Max DNS label length
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !name.starts_with('-')
        && !name.starts_with('.')
}

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

        let Ok(xml) = std::str::from_utf8(&output.stdout) else { return Vec::new(); };
        parse_gpu_passthrough_xml(xml)
    }

    fn get_vm_gpu_mappings(vm_name: &str) -> (Vec<String>, Vec<(String, String)>) {
        if !is_valid_vm_name(vm_name) { return (Vec::new(), Vec::new()); }
        let Ok(output) = Command::new("virsh").args(["dumpxml", vm_name]).output() else { return (Vec::new(), Vec::new()); };
        if !output.status.success() { return (Vec::new(), Vec::new()); }
        let Ok(xml) = std::str::from_utf8(&output.stdout) else { return (Vec::new(), Vec::new()); };
        (parse_gpu_passthrough_xml(xml), parse_vm_gpu_mappings_xml(xml))
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

    /// Collect only telemetry tied to an unambiguous guest-to-host PCI mapping.
    pub fn get_vm_gpu_metrics(
        vm_name: &str, mappings: &VmGpuMappings, enable_register_access: bool,
    ) -> Vec<GpuMetrics> {
        Self::get_vm_gpu_metrics_with(vm_name, mappings, enable_register_access, &mut Self::guest_exec)
    }

    fn get_vm_gpu_metrics_with(
        vm_name: &str,
        mappings: &VmGpuMappings,
        _enable_register_access: bool,
        guest_exec: &mut impl FnMut(&str, &str, &[&str]) -> Option<(String, i32)>,
    ) -> Vec<GpuMetrics> {
        let mut gpus = Vec::new();

        // Run nvidia-smi inside VM
        let nvidia_smi_args = [
            "--query-gpu=index,name,uuid,pci.bus_id,temperature.gpu,fan.speed,power.draw,utilization.gpu,utilization.memory,memory.used,memory.total,clocks.sm,clocks.mem,driver_version",
            "--format=csv,noheader,nounits"
        ];

        let (stdout, exit_code) = match guest_exec(vm_name, "/usr/bin/nvidia-smi", &nvidia_smi_args) {
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

        // No guest BAR profile has accepted qualification evidence. Even with
        // register access enabled, nvidia-smi is the only permitted guest query.
        let mut guest_counts = HashMap::<String, usize>::new();
        let mut uuid_counts = HashMap::<String, usize>::new();
        for line in stdout.lines() {
            let parts: Vec<_> = line.split(',').map(str::trim).collect();
            if parts.len() < 14 { continue; }
            if let Some(guest) = canonical_pci_bus_id(parts[3]) {
                *guest_counts.entry(guest).or_default() += 1;
            }
            *uuid_counts.entry(parts[2].to_string()).or_default() += 1;
        }

        // Parse nvidia-smi output
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
            if parts.len() < 14 {
                continue;
            }

            let Some(vm_pci_bus) = canonical_pci_bus_id(parts[3]) else { continue; };
            if guest_counts.get(&vm_pci_bus) != Some(&1)
                || parts[2].is_empty() || uuid_counts.get(parts[2]) != Some(&1)
            { continue; }
            let Some((host_bdf, host_gpu_idx, device_id)) = mappings.get(&vm_pci_bus) else { continue; };
            if canonical_pci_bus_id(host_bdf).as_deref() != Some(host_bdf.as_str())
                || mappings.values().filter(|(host, _, _)| host == host_bdf).count() != 1
            { continue; }

            let mut gpu = GpuMetrics {
                index: *host_gpu_idx,
                name: parts[1].to_string(),
                uuid: parts[2].to_string(),
                pci_bus_id: host_bdf.clone(),
                device_id: *device_id,
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
                
                vram_temp: None,
                hotspot_temp: None,
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

    /// Collect metrics only for uniquely mapped GPUs in running domains.
    pub fn collect_all(
        host_gpu_map: &HashMap<String, (u32, u16)>, enable_register_access: bool,
    ) -> Vec<GpuMetrics> {
        if !Self::is_available() { return Vec::new(); }
        let mut domains: Vec<VmGpuDomain> = Self::get_running_vms().into_iter().map(|vm| {
            let (sources, mappings) = Self::get_vm_gpu_mappings(&vm);
            let mappings = mappings.into_iter().filter_map(|(host, guest)| {
                host_gpu_map.get(&host).map(|(index, device)| (guest, (host.clone(), *index, *device)))
            }).collect();
            (vm, sources, mappings)
        }).collect();
        retain_unique_vm_assignments(&mut domains);
        let mut metrics = Vec::new();
        for (vm, _, mappings) in domains {
            if !mappings.is_empty() {
                metrics.extend(Self::get_vm_gpu_metrics(&vm, &mappings, enable_register_access));
            }
        }
        metrics
    }
}

fn retain_unique_vm_assignments(domains: &mut [VmGpuDomain]) {
    let mut counts = HashMap::<String, usize>::new();
    for (_, sources, _) in domains.iter() {
        for host in sources { *counts.entry(host.clone()).or_default() += 1; }
    }
    for (_, _, mappings) in domains {
        mappings.retain(|_, (host, _, _)| counts.get(host) == Some(&1));
    }
}

#[derive(Default)]
struct VmPciAssignment {
    host: Option<String>,
    guest: Option<String>,
    source_addresses: usize,
    guest_addresses: usize,
}

fn xml_attributes(element: &BytesStart<'_>, version: XmlVersion) -> Option<HashMap<String, String>> {
    let mut attributes = HashMap::new();
    for attribute in element.attributes() {
        let attribute = attribute.ok()?;
        let key = attribute.key.into_inner().to_string();
        let value = attribute.normalized_value(version).ok()?.into_owned();
        if attributes.insert(key, value).is_some() { return None; }
    }
    Some(attributes)
}

fn pci_address_attributes(attributes: &HashMap<String, String>) -> Option<String> {
    let number = |name: &str, maximum: u32| -> Option<u32> {
        let value = attributes.get(name)?;
        let value = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")).unwrap_or(value);
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
        let value = u32::from_str_radix(value, 16).ok()?;
        (value <= maximum).then_some(value)
    };
    Some(format!("{:04x}:{:02x}:{:02x}.{}", number("domain", 0xffff)?, number("bus", 0xff)?, number("slot", 0x1f)?, number("function", 7)?))
}

// TRACEWEAVER: entrypoint=parse_vm_hostdev_xml; req=VM-FIX-002,VM-FIX-003; trace=TRACE-VM-002,TRACE-VM-003
fn parse_vm_hostdev_xml(xml: &str) -> Option<Vec<VmPciAssignment>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().enable_all_checks(true);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = Vec::<String>::new();
    let mut roots = 0;
    let mut declared = false;
    let mut version = XmlVersion::Implicit1_0;
    let mut current: Option<VmPciAssignment> = None;
    let mut assignments = Vec::new();
    loop {
        match reader.read_event().ok()? {
            Event::Decl(declaration) => {
                if roots != 0 || declared { return None; }
                declared = true;
                version = declaration.xml_version().ok()?;
                if let Some(encoding) = declaration.encoding() {
                    let encoding = encoding.ok()?;
                    if !encoding.eq_ignore_ascii_case("utf-8") && !encoding.eq_ignore_ascii_case("us-ascii") { return None; }
                }
            }
            Event::DocType(_) => return None,
            Event::Start(element) => {
                let name = element.name().into_inner().to_string();
                let attributes = xml_attributes(&element, version)?;
                if stack.is_empty() {
                    roots += 1;
                    if roots != 1 || name != "domain" { return None; }
                }
                if stack.iter().map(String::as_str).eq(["domain", "devices"])
                    && name == "hostdev" && attributes.get("type").map(String::as_str) == Some("pci")
                { current = Some(VmPciAssignment::default()); }
                if let Some(assignment) = current.as_mut() {
                    if name == "address" && stack.iter().map(String::as_str).eq(["domain", "devices", "hostdev", "source"]) {
                        assignment.source_addresses += 1;
                        assignment.host = pci_address_attributes(&attributes);
                    } else if name == "address" && stack.iter().map(String::as_str).eq(["domain", "devices", "hostdev"]) {
                        assignment.guest_addresses += 1;
                        assignment.guest = (attributes.get("type").map(String::as_str) == Some("pci"))
                            .then(|| pci_address_attributes(&attributes)).flatten();
                    }
                }
                stack.push(name);
            }
            Event::End(element) => {
                if stack.pop().as_deref() != Some(element.name().into_inner()) { return None; }
                if element.name().into_inner() == "hostdev"
                    && stack.iter().map(String::as_str).eq(["domain", "devices"])
                {
                    if let Some(assignment) = current.take() { assignments.push(assignment); }
                }
            }
            Event::Text(text) => { if stack.is_empty() && !text.trim().is_empty() { return None; } }
            Event::GeneralRef(reference) => {
                let character = reference.resolve_char_ref().ok()?;
                let predefined = matches!(&*reference, "amp" | "lt" | "gt" | "apos" | "quot");
                if stack.is_empty() || (!predefined && character.is_none()) { return None; }
            }
            Event::CData(_) if stack.is_empty() => return None,
            Event::Eof => break,
            _ => (),
        }
    }
    (roots == 1 && stack.is_empty()).then_some(assignments)
}

/// Assignment evidence uses only one direct hostdev/source PCI address.
fn parse_gpu_passthrough_xml(xml: &str) -> Vec<String> {
    parse_vm_hostdev_xml(xml).unwrap_or_default().into_iter().filter_map(|assignment| {
        assignment.host.filter(|host| assignment.source_addresses == 1 && host.ends_with(".0"))
    }).collect()
}

// TRACEWEAVER: entrypoint=parse_vm_gpu_mappings_xml; req=VM-FIX-002,VM-FIX-003; trace=TRACE-VM-002,TRACE-VM-003
fn parse_vm_gpu_mappings_xml(xml: &str) -> Vec<(String, String)> {
    let assignments = parse_vm_hostdev_xml(xml).unwrap_or_default();
    let mut hosts = HashMap::<&str, usize>::new();
    let mut guests = HashMap::<&str, usize>::new();
    for assignment in &assignments {
        if let Some(host) = &assignment.host { *hosts.entry(host.as_str()).or_default() += 1; }
        if let Some(guest) = &assignment.guest { *guests.entry(guest.as_str()).or_default() += 1; }
    }
    assignments.iter().filter_map(|assignment| {
        let host = assignment.host.as_ref()?;
        let guest = assignment.guest.as_ref()?;
        (assignment.source_addresses == 1 && assignment.guest_addresses == 1 && host.ends_with(".0")
            && hosts.get(host.as_str()) == Some(&1) && guests.get(guest.as_str()) == Some(&1))
            .then(|| (host.clone(), guest.clone()))
    }).collect()
}

#[cfg(test)]
fn parse_pci_address(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    match reader.read_event().ok()? {
        Event::Empty(address) if address.name().into_inner() == "address" =>
            pci_address_attributes(&xml_attributes(&address, XmlVersion::Implicit1_0)?),
        _ => None,
    }
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

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_definition_libvirt_guest_target_is_not_host_assignment_evidence() {
        let xml = r#"<domain><devices>
<hostdev mode='subsystem' type='pci' managed='yes'>
  <source>
    <address domain='0x0000' bus='0xc1' slot='0x00' function='0x0'/>
  </source>
  <address type='pci' domain='0x0000' bus='0x01' slot='0x00' function='0x0'/>
</hostdev>
</devices></domain>"#;
        assert_eq!(parse_gpu_passthrough_xml(xml), vec!["0000:c1:00.0"]);
    }

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_definition_libvirt_source_parsing_accepts_xml_layout_and_quote_variants() {
        for xml in [
            r#"<domain><devices><hostdev type="pci"><source><address function="0x0" slot="0x00" bus="0xc1" domain="0x0000"/></source></hostdev></devices></domain>"#,
            "<domain>\n<devices>\n<hostdev\n type='pci'>\n<source>\n<address\n domain='0x0000'\n bus='0xc1'\n slot='0x00'\n function='0x0'/>\n</source>\n</hostdev>\n</devices>\n</domain>",
        ] {
            assert_eq!(parse_gpu_passthrough_xml(xml), vec!["0000:c1:00.0"], "{xml}");
        }
    }

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_definition_libvirt_rejects_malformed_xml_and_invalid_source_bdf() {
        for xml in [
            "<hostdev type='pci'>\n<source>\n<address domain='0x0000' bus='0xc1' slot='0x00' function='0x0'/>",
            "<domain><devices>\n<hostdev type='pci'>\n<source>\n<address domain='0x0000' bus='0xzz' slot='0x00' function='0x0'/>\n</source>\n</hostdev>\n</devices></domain>",
            "<domain><devices>\n<hostdev type='pci'>\n<source>\n<address domain='0x0000' bus='0xc1' slot='0x20' function='0x0'/>\n</source>\n</hostdev>\n</devices></domain>",
        ] {
            assert!(parse_gpu_passthrough_xml(xml).is_empty(), "{xml}");
        }
    }

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_definition_libvirt_guest_only_address_proves_no_host_assignment() {
        let xml = "<domain><devices>\n<hostdev type='pci'>\n<address type='pci' domain='0x0000' bus='0x01' slot='0x00' function='0x0'/>\n</hostdev>\n</devices></domain>";
        assert!(parse_gpu_passthrough_xml(xml).is_empty());
    }

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_definition_libvirt_valid_source_remains_detectable_without_guest_target() {
        let xml = "<domain><devices>\n<hostdev type='pci'>\n<source>\n<address domain='0x0001' bus='0xc1' slot='0x00' function='0x0'/>\n</source>\n</hostdev>\n</devices></domain>";
        assert_eq!(parse_gpu_passthrough_xml(xml), vec!["0001:c1:00.0"]);
    }

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

    fn vm_definition_csv(index: u32, guest_bdf: &str, uuid: &str, temp: u32) -> String {
        format!("{index}, NVIDIA fixture, {uuid}, {guest_bdf}, {temp}, 30, 100.0, 20, 10, 100, 1000, 1200, 2000, 590.48.01")
    }

    // TRACEWEAVER: verifies=VER-VM-003; req=VM-FIX-003; trace=TRACE-VM-003
    #[test]
    fn vm_definition_reordered_guest_indexes_map_exact_bdf_without_ordinal_fallback() {
        let mappings = HashMap::from([
            ("0000:01:00.0".into(), ("0000:c1:00.0".into(), 7, 0x2204)),
            ("0000:02:00.0".into(), ("0000:41:00.0".into(), 3, 0x2204)),
        ]);
        let csv = [vm_definition_csv(0, "00000000:02:00.0", "GPU-B", 62),
            vm_definition_csv(1, "00000000:01:00.0", "GPU-A", 51),
            vm_definition_csv(7, "00000000:03:00.0", "GPU-UNMAPPED", 99)].join("\n");
        let mut calls = Vec::new();
        let rows = VmMetricsCollector::get_vm_gpu_metrics_with("reserved-vm", &mappings, false,
            &mut |vm, command, args| {
                calls.push((vm.to_string(), command.to_string(), args.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
                Some((csv.clone(), 0))
            });
        assert_eq!(rows.len(), 2);
        for (uuid, index, host_bdf, temp) in [
            ("GPU-A", 7, "0000:c1:00.0", 51), ("GPU-B", 3, "0000:41:00.0", 62),
        ] {
            let row = rows.iter().find(|r| r.uuid == uuid).unwrap();
            assert_eq!(row.index, index);
            assert_eq!(row.pci_bus_id, host_bdf);
            assert_eq!(row.gpu_temp, Some(temp));
            assert_eq!(row.source, MetricSource::Vm);
        }
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "/usr/bin/nvidia-smi");
    }

    // TRACEWEAVER: verifies=VER-VM-003; req=VM-FIX-003; trace=TRACE-VM-003
    #[test]
    fn vm_definition_duplicate_guest_rows_are_not_arbitrarily_selected() {
        let mappings = HashMap::from([("0000:01:00.0".into(), ("0000:c1:00.0".into(), 7, 0x2204))]);
        let csv = [vm_definition_csv(0, "0000:01:00.0", "GPU-A", 51),
            vm_definition_csv(1, "00000000:01:00.0", "GPU-B", 99)].join("\n");
        let rows = VmMetricsCollector::get_vm_gpu_metrics_with("reserved-vm", &mappings, false,
            &mut |_, _, _| Some((csv.clone(), 0)));
        assert!(rows.is_empty(), "conflicting guest rows must retain assignment-only status");
    }

    // TRACEWEAVER: verifies=VER-VM-003; req=VM-FIX-003; trace=TRACE-VM-003
    #[test]
    fn vm_definition_xml_mapping_rejects_duplicate_missing_and_conflicting_targets() {
        let entry = |host: &str, guest: &str| format!(
            "<hostdev type='pci'><source><address domain='0x0000' bus='0x{host}' slot='0x00' function='0x0'/></source><address type='pci' domain='0x0000' bus='0x{guest}' slot='0x00' function='0x0'/></hostdev>");
        let wrap = |entries: String| format!("<domain><devices>{entries}</devices></domain>");
        let valid = wrap(format!("{}{}", entry("c1", "02"), entry("41", "01")));
        assert_eq!(parse_vm_gpu_mappings_xml(&valid), vec![
            ("0000:c1:00.0".to_string(), "0000:02:00.0".to_string()),
            ("0000:41:00.0".to_string(), "0000:01:00.0".to_string()),
        ]);
        for entries in [
            format!("{}{}", entry("c1", "01"), entry("41", "01")),
            format!("{}{}", entry("c1", "01"), entry("c1", "02")),
            format!("{}{}", entry("c1", "01"), entry("c1", "01")),
            "<hostdev type='pci'><source><address domain='0x0000' bus='0xc1' slot='0x00' function='0x0'/></source></hostdev>".into(),
            entry("c1", "zz"),
        ] {
            assert!(parse_vm_gpu_mappings_xml(&wrap(entries)).is_empty());
        }
    }

    // TRACEWEAVER: verifies=VER-VM-005; req=VM-FIX-005; trace=TRACE-VM-005
    #[test]
    fn vm_definition_guest_command_reader_honors_no_register_and_unsupported_profiles() {
        for (device_id, enabled) in [(0x2204, false), (0x2b85, true), (0x26b2, true), (0xffff, true)] {
            let mappings = HashMap::from([("0000:01:00.0".into(), ("0000:c1:00.0".into(), 7, device_id))]);
            let csv = vm_definition_csv(0, "00000000:01:00.0", "GPU-A", 51);
            let mut calls = Vec::new();
            let rows = VmMetricsCollector::get_vm_gpu_metrics_with("reserved-vm", &mappings, enabled,
                &mut |vm, command, args| {
                    calls.push((vm.to_string(), command.to_string(), args.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
                    if command == "/usr/bin/nvidia-smi" { Some((csv.clone(), 0)) }
                    else { Some((r#"[{"p":"0000:01:00.0","v":73,"h":81}]"#.into(), 0)) }
                });
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].gpu_temp, Some(51));
            assert_eq!(rows[0].vram_temp, None);
            assert_eq!(rows[0].hotspot_temp, None);
            assert_eq!(calls.len(), 1, "device={device_id:x}, enabled={enabled}, calls={calls:?}");
            assert_eq!(calls[0].1, "/usr/bin/nvidia-smi");
        }
    }

    // TRACEWEAVER: verifies=VER-VM-005; req=VM-FIX-005; trace=TRACE-VM-005
    #[test]
    fn vm_definition_unvalidated_3090_guest_profile_never_invokes_bar_callback() {
        let mappings = HashMap::from([("0000:01:00.0".into(), ("0000:c1:00.0".into(), 7, 0x2204))]);
        let csv = vm_definition_csv(0, "00000000:01:00.0", "GPU-A", 51);
        let mut calls = Vec::new();
        let rows = VmMetricsCollector::get_vm_gpu_metrics_with("reserved-vm", &mappings, true,
            &mut |_, command, args| {
                calls.push((command.to_string(), args.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
                if command == "/usr/bin/nvidia-smi" { Some((csv.clone(), 0)) }
                else { Some((r#"[{"p":"0000:01:00.0","v":73,"h":81},{"p":"0000:11:00.0","v":99,"h":99}]"#.into(), 0)) }
            });
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].gpu_temp, Some(51));
        assert_eq!(rows[0].vram_temp, None);
        assert_eq!(rows[0].hotspot_temp, None);
        assert_eq!(calls.len(), 1, "device ID alone cannot establish a validated guest BAR profile");
        assert_eq!(calls[0].0, "/usr/bin/nvidia-smi");
    }

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_definition_xml_rejects_dtd_encoding_and_non_direct_source_spoofs() {
        let source = "<source><address domain='0x0000' bus='0xc1' slot='0x00' function='0x0'/></source>";
        let domain = format!("<domain><devices><hostdev type='pci'>{source}</hostdev></devices></domain>");
        for xml in [
            format!("<!DOCTYPE domain SYSTEM 'https://invalid.example/domain.dtd'>{domain}"),
            format!("<?xml version='1.0' encoding='UTF-16'?>{domain}"),
            format!("<domain><metadata><hostdev type='pci'>{source}</hostdev></metadata></domain>"),
            format!("<domain><devices><hostdev type='pci'><metadata>{source}</metadata></hostdev></devices></domain>"),
            format!("{domain}{domain}"),
            "<domain><devices><hostdev type='pci'><source><address domain='0x0000' bus='0xc1' bus='0x41' slot='0x00' function='0x0'/></source></hostdev></devices></domain>".into(),
        ] {
            assert!(parse_gpu_passthrough_xml(&xml).is_empty(), "{xml}");
        }
        assert_eq!(parse_gpu_passthrough_xml(&format!("<?xml version='1.0' encoding='UTF-8'?>{domain}")), vec!["0000:c1:00.0"]);
    }

    // TRACEWEAVER: verifies=VER-VM-003; req=VM-FIX-003; trace=TRACE-VM-003
    #[test]
    fn vm_definition_cross_domain_host_assignment_does_not_choose_a_guest() {
        let mut domains = vec![
            ("guest-a".into(), vec!["0000:c1:00.0".into()], HashMap::from([("0000:01:00.0".into(), ("0000:c1:00.0".into(), 0, 0x2204))])),
            ("guest-b".into(), vec!["0000:c1:00.0".into(), "0000:41:00.0".into(), "0000:e3:00.0".into()], HashMap::from([
                ("0000:02:00.0".into(), ("0000:c1:00.0".into(), 0, 0x2204)),
                ("0000:03:00.0".into(), ("0000:41:00.0".into(), 1, 0x2204)),
                ("0000:04:00.0".into(), ("0000:e3:00.0".into(), 2, 0x2204)),
            ])),
            ("source-only-guest".into(), vec!["0000:e3:00.0".into()], HashMap::new()),
        ];
        retain_unique_vm_assignments(&mut domains);
        assert!(domains[0].2.is_empty());
        assert_eq!(domains[1].2.len(), 1);
        assert_eq!(domains[1].2["0000:03:00.0"].0, "0000:41:00.0");
        assert!(domains[2].2.is_empty());
    }

    fn vm_review_xml_address(bus: &str, target: bool) -> String {
        format!("<address {}domain='0x0000' bus='0x{bus}' slot='0x00' function='0x0'/>",
            if target { "type='pci' " } else { "" })
    }

    // TRACEWEAVER: verifies=VER-VM-003; req=VM-FIX-003; trace=TRACE-VM-003
    #[test]
    fn vm_review_duplicate_guest_target_cannot_erase_another_host_conflict() {
        let valid = format!("<hostdev type='pci'><source>{}</source>{}</hostdev>",
            vm_review_xml_address("c1", false), vm_review_xml_address("01", true));
        for targets in [["01", "02"], ["02", "01"]] {
            let malformed = format!("<hostdev type='pci'><source>{}</source>{}{}</hostdev>",
                vm_review_xml_address("41", false), vm_review_xml_address(targets[0], true), vm_review_xml_address(targets[1], true));
            let xml = format!("<domain><devices>{valid}{malformed}</devices></domain>");
            assert!(parse_vm_gpu_mappings_xml(&xml).is_empty(), "{targets:?}: {xml}");
            assert_eq!(parse_gpu_passthrough_xml(&xml), vec!["0000:c1:00.0", "0000:41:00.0"]);
        }
    }

    // TRACEWEAVER: verifies=VER-VM-003; req=VM-FIX-003; trace=TRACE-VM-003
    #[test]
    fn vm_review_duplicate_host_source_cannot_erase_another_mapping_conflict() {
        let valid = format!("<hostdev type='pci'><source>{}</source>{}</hostdev>",
            vm_review_xml_address("c1", false), vm_review_xml_address("01", true));
        for sources in [["c1", "41"], ["41", "c1"]] {
            let malformed = format!("<hostdev type='pci'><source>{}{}</source>{}</hostdev>",
                vm_review_xml_address(sources[0], false), vm_review_xml_address(sources[1], false), vm_review_xml_address("02", true));
            let xml = format!("<domain><devices>{valid}{malformed}</devices></domain>");
            assert!(parse_vm_gpu_mappings_xml(&xml).is_empty(), "{sources:?}: {xml}");
        }
        let source_only = format!("<domain><devices><hostdev type='pci'><source>{}</source></hostdev><hostdev type='pci'><source>{}</source></hostdev></devices></domain>",
            vm_review_xml_address("c1", false), vm_review_xml_address("41", false));
        assert_eq!(parse_gpu_passthrough_xml(&source_only), vec!["0000:c1:00.0", "0000:41:00.0"]);
        assert!(parse_vm_gpu_mappings_xml(&source_only).is_empty());
    }

    fn vm_review_valid_xml_domain(metadata: &str) -> String {
        format!("<domain>{metadata}<devices><hostdev type='pci'><source>{}</source>{}</hostdev></devices></domain>",
            vm_review_xml_address("c1", false), vm_review_xml_address("01", true))
    }

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_review_xml_declaration_must_precede_whitespace_comment_or_pi() {
        // XML1.0 productions [22] prolog and [23] XMLDecl: declaration precedes Misc.
        let declaration = "<?xml version='1.0' encoding='UTF-8'?>";
        let domain = vm_review_valid_xml_domain("");
        for prefix in [" ", "\n", "<!-- before declaration -->", "<?fixture before?>"] {
            let xml = format!("{prefix}{declaration}{domain}");
            assert!(parse_gpu_passthrough_xml(&xml).is_empty(), "{xml:?}");
            assert!(parse_vm_gpu_mappings_xml(&xml).is_empty(), "{xml:?}");
        }
    }

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_review_xml_illegal_literal_characters_reject_entire_document() {
        // XML1.0 production [2] Char excludes these code points everywhere.
        for character in ['\u{0}', '\u{1}', '\u{b}', '\u{c}', '\u{1f}', '\u{fffe}', '\u{ffff}'] {
            for metadata in [format!("<name>{character}</name>"), format!("<metadata label='{character}'/>")] {
                let xml = format!("<?xml version='1.0'?>{}", vm_review_valid_xml_domain(&metadata));
                assert!(parse_gpu_passthrough_xml(&xml).is_empty(), "{xml:?}");
                assert!(parse_vm_gpu_mappings_xml(&xml).is_empty(), "{xml:?}");
            }
        }
    }

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_review_xml_illegal_numeric_references_reject_entire_document() {
        // XML1.0 section4.1 Legal Character constraint also applies to references.
        for reference in ["&#0;", "&#x1;", "&#xB;", "&#x1F;", "&#xFFFE;", "&#xFFFF;"] {
            for metadata in [format!("<name>{reference}</name>"), format!("<metadata label='{reference}'/>")] {
                let xml = format!("<?xml version='1.0'?>{}", vm_review_valid_xml_domain(&metadata));
                assert!(parse_gpu_passthrough_xml(&xml).is_empty(), "{xml:?}");
                assert!(parse_vm_gpu_mappings_xml(&xml).is_empty(), "{xml:?}");
            }
        }
    }

    // TRACEWEAVER: verifies=VER-VM-002; req=VM-FIX-002; trace=TRACE-VM-002
    #[test]
    fn vm_review_valid_utf8_bom_declaration_and_legal_characters_remain_supported() {
        let domain = vm_review_valid_xml_domain("<name>é😀\t\n\r&#x9;&#xA;&#xD;&#x20;&#x10000;&amp;</name>");
        for prefix in ["", "\u{feff}", "<?xml version='1.0' encoding='UTF-8'?>", "\u{feff}<?xml version='1.0' encoding='UTF-8'?>"] {
            let xml = format!("{prefix}{domain}");
            assert_eq!(parse_gpu_passthrough_xml(&xml), vec!["0000:c1:00.0"], "{xml:?}");
            assert_eq!(parse_vm_gpu_mappings_xml(&xml), vec![("0000:c1:00.0".into(), "0000:01:00.0".into())], "{xml:?}");
        }
    }

    // HELD future case, not an executable expectation: a separately hardware-qualified
    // guest BAR profile may eventually have a BDF-scoped positive reader control.
    // The currently validated guest-profile inventory is empty, including RTX3090.
    // No injectable policy or device-ID-only enablement is authorized by this fixture.

}
