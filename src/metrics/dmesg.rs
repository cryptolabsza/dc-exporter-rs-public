//! Dmesg GPU Error Parser
//!
//! Parses kernel ring buffer (dmesg) for NVIDIA GPU hardware failure indicators.
//! Key error patterns that indicate hardware failure vs normal operation:
//!
//! - `NVRM: GPU <pci_id>: RmInitAdapter failed!` - GPU initialization failure (defective GPU)
//! - `NVRM: GPU <pci_id>: rm_init_adapter failed` - Same as above (different format)
//! - `Xid (<pci_id>): <code>` - NVIDIA XID error (various severity)
//! - `GPU has fallen off the bus` - GPU completely unresponsive
//! - `GPU at PCI <pci_id> has fallen off the bus` - GPU fallen off bus with PCI ID
//!
//! These errors are NEVER present during normal VM passthrough (vfio-pci),
//! making them a strong differentiator for hardware failure.

use std::collections::HashMap;
use std::process::Command;
use tracing::{debug, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DmesgSource {
    FlaggedDmesg,
    PlainDmesg,
    KernLog,
}

/// GPU errors found in dmesg, keyed by PCI bus ID
#[derive(Debug, Clone, Default)]
pub struct DmesgGpuErrors {
    /// Whether a kernel log source was read successfully. An empty successful
    /// read is distinct from a failed/unreadable log.
    pub read_succeeded: bool,

    /// Stable identity of the kernel-log corpus used for this read.
    pub source: Option<DmesgSource>,

    /// Per-GPU errors keyed by PCI bus ID (e.g., "0000:c2:00.0")
    pub per_gpu: HashMap<String, GpuDmesgInfo>,
    
    /// Whether any GPU has fallen off the bus (system-wide)
    pub any_fallen_off_bus: bool,
}

/// Dmesg error information for a single GPU
#[derive(Debug, Clone, Default)]
pub struct GpuDmesgInfo {
    /// RmInitAdapter failed count (strong hardware failure signal)
    pub rm_init_adapter_failed: u32,
    
    /// XID error codes seen
    pub xid_errors: Vec<u32>,
    
    /// Whether this GPU has fallen off the bus
    pub fallen_off_bus: bool,
    
    /// Raw error messages (last N for debugging)
    pub recent_errors: Vec<String>,
    
    /// Whether this GPU has confirmed hardware failure based on dmesg
    pub is_hardware_failure: bool,
    
    /// Human-readable failure summary
    pub failure_summary: Option<String>,
}

/// Critical XID error codes that indicate hardware failure
/// Reference: https://docs.nvidia.com/deploy/xid-errors/
pub const HARDWARE_FAILURE_XIDS: &[u32] = &[
    48,  // Double Bit ECC Error
    63,  // ECC page retirement / Row Remapper failure
    64,  // ECC page retirement / Row Remapper failure
    69,  // Graphics Engine Exception (often hardware)
    79,  // GPU has fallen off the bus
    92,  // High single-bit ECC error rate
    94,  // Contained ECC error (may indicate degrading hardware)
    95,  // Uncontained ECC error
];

/// Parse dmesg for NVIDIA GPU errors
/// Returns per-GPU error information for hardware failure detection
pub fn parse_dmesg_gpu_errors() -> DmesgGpuErrors {
    parse_first_available_source(
        || read_dmesg_command(&["--level=err,warn", "--facility=kern"], "flagged dmesg"),
        || read_dmesg_command(&[], "plain dmesg"),
        || match std::fs::read_to_string("/var/log/kern.log") {
            Ok(content) => Some(content),
            Err(error) => {
                warn!("Failed to read kernel logs: {}", error);
                None
            }
        },
    )
}

fn read_dmesg_command(args: &[&str], description: &str) -> Option<String> {
    match Command::new("dmesg").args(args).output() {
        Ok(output) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => {
            debug!("{} exited with {}, trying fallback", description, output.status);
            None
        }
        Err(error) => {
            debug!("{} failed ({}), trying fallback", description, error);
            None
        }
    }
}

fn parse_first_available_source<Flagged, Plain, KernLog>(
    flagged_dmesg: Flagged,
    plain_dmesg: Plain,
    kern_log: KernLog,
) -> DmesgGpuErrors
where
    Flagged: FnOnce() -> Option<String>,
    Plain: FnOnce() -> Option<String>,
    KernLog: FnOnce() -> Option<String>,
{
    let selected = flagged_dmesg()
        .map(|content| (DmesgSource::FlaggedDmesg, content))
        .or_else(|| plain_dmesg().map(|content| (DmesgSource::PlainDmesg, content)))
        .or_else(|| kern_log().map(|content| (DmesgSource::KernLog, content)));

    let Some((source, content)) = selected else {
        return DmesgGpuErrors::default();
    };

    let mut errors = DmesgGpuErrors {
        source: Some(source),
        ..Default::default()
    };
    parse_lines(&content, &mut errors);
    errors
}

/// Parse dmesg output lines for GPU errors
pub(super) fn parse_lines(content: &str, errors: &mut DmesgGpuErrors) {
    errors.read_succeeded = true;

    for line in content.lines() {
        let line_upper = line.to_uppercase();
        
        // Skip lines that don't mention NVIDIA/NVRM
        if !line_upper.contains("NVRM") && !line_upper.contains("NVIDIA") && !line_upper.contains("XID") {
            continue;
        }
        
        // Pattern 1: RmInitAdapter failed
        // "NVRM: GPU 0000:c2:00.0: RmInitAdapter failed! (0x62:0x65:2661)"
        if line.contains("RmInitAdapter failed") || line.contains("rm_init_adapter failed") {
            if let Some(pci_id) = extract_pci_id_from_nvrm(line) {
                let entry = errors.per_gpu.entry(pci_id).or_default();
                entry.rm_init_adapter_failed += 1;
                entry.is_hardware_failure = true;
                push_recent_error(entry, line);
            }
        }
        
        // Pattern 2: GPU has fallen off the bus
        // "NVRM: GPU at PCI:0000:c2:00.0 has fallen off the bus"
        // "NVRM: GPU 0000:c2:00.0: GPU has fallen off the bus"
        if line_upper.contains("FALLEN OFF THE BUS") {
            errors.any_fallen_off_bus = true;
            if let Some(pci_id) = extract_pci_id_from_nvrm(line) {
                let entry = errors.per_gpu.entry(pci_id).or_default();
                entry.fallen_off_bus = true;
                entry.is_hardware_failure = true;
                push_recent_error(entry, line);
            }
        }
        
        // Pattern 3: XID errors
        // "NVRM: Xid (PCI:0000:c2:00.0): 79, pid=1234, GPU has fallen off the bus"
        // "nvidia 0000:c2:00.0: Xid (0000:c2:00.0): 48, ..."
        if line_upper.contains("XID") {
            if let Some((pci_id, xid_code)) = extract_xid_error(line) {
                let entry = errors.per_gpu.entry(pci_id).or_default();
                entry.xid_errors.push(xid_code);
                
                // Check if this XID indicates hardware failure
                if HARDWARE_FAILURE_XIDS.contains(&xid_code) {
                    entry.is_hardware_failure = true;
                }
                
                push_recent_error(entry, line);
            }
        }
    }
    
    // Generate failure summaries
    for (pci_id, info) in errors.per_gpu.iter_mut() {
        if info.is_hardware_failure {
            let mut reasons = Vec::new();
            
            if info.rm_init_adapter_failed > 0 {
                reasons.push(format!(
                    "RmInitAdapter failed {}x",
                    info.rm_init_adapter_failed
                ));
            }
            
            if info.fallen_off_bus {
                reasons.push("GPU fallen off bus".to_string());
            }
            
            let hw_xids: Vec<u32> = info.xid_errors.iter()
                .filter(|x| HARDWARE_FAILURE_XIDS.contains(x))
                .copied()
                .collect();
            if !hw_xids.is_empty() {
                reasons.push(format!("XID errors: {:?}", hw_xids));
            }
            
            info.failure_summary = Some(format!("{} - {}", pci_id, reasons.join(", ")));
            
            debug!(
                "GPU {} confirmed hardware failure: {}",
                pci_id,
                info.failure_summary.as_ref().unwrap()
            );
        }
    }
}

/// Extract PCI bus ID from NVRM log line
/// Matches patterns like "GPU 0000:c2:00.0:" or "PCI:0000:c2:00.0"
fn extract_pci_id_from_nvrm(line: &str) -> Option<String> {
    // Pattern: xxxx:xx:xx.x (PCI bus ID format)
    let re_patterns = [
        // "GPU 0000:c2:00.0:" - with or without leading "GPU "
        "GPU ",
        "PCI:",
        "PCI:0000:",
        "nvidia ",
    ];
    
    for pattern in &re_patterns {
        if let Some(pos) = line.find(pattern) {
            let after = &line[pos + pattern.len()..];
            if let Some(pci_id) = extract_pci_bus_id(after) {
                return Some(pci_id);
            }
        }
    }
    
    // Fallback: just find any PCI bus ID pattern in the line
    extract_pci_bus_id(line)
}

/// Parse the PCI token belonging to Xid, never another address elsewhere in a log.
fn extract_xid_error(line:&str)->Option<(String,u32)> {
    let lower=line.to_ascii_lowercase();let start=lower.find("xid")?;
    let after=&line[start+3..];let open=after.find('(')?;let close=after[open+1..].find(')')?+open+1;
    let raw=&after[open+1..close];
    let raw=if raw.get(..4).is_some_and(|s|s.eq_ignore_ascii_case("pci:")) {&raw[4..]}else{raw};
    let pci=canonical_pci_token(raw)?;
    let value=after[close+1..].strip_prefix(':')?.trim_start();
    let end=value.find(|c:char|!c.is_ascii_digit()).unwrap_or(value.len());
    if end==0 {return None;}let code=value[..end].parse().ok()?;
    Some((pci,code))
}
/// Canonical full or functionless token; native NVML may use an eight-digit domain.
pub(super) fn canonical_pci_token(raw:&str)->Option<String> {
    let fields:Vec<_>=raw.split(':').collect();if fields.len()!=3 {return None;}
    if !matches!(fields[0].len(),4|8) || fields[1].len()!=2 {return None;}
    if !fields[0].bytes().all(|b|b.is_ascii_hexdigit()) || !fields[1].bytes().all(|b|b.is_ascii_hexdigit()) {return None;}
    let domain=u32::from_str_radix(fields[0],16).ok()?;if domain>0xffff {return None;}
    let bus=u8::from_str_radix(fields[1],16).ok()?;
    let parts:Vec<_>=fields[2].split('.').collect();if !(parts.len()==1||parts.len()==2) || parts[0].len()!=2 {return None;}
    if !parts[0].bytes().all(|b|b.is_ascii_hexdigit()) {return None;}
    let device=u8::from_str_radix(parts[0],16).ok()?;if device>31 {return None;}
    let base=format!("{domain:04x}:{bus:02x}:{device:02x}");
    if parts.len()==1 {return Some(base);}
    if parts[1].len()!=1 {return None;}let function=parts[1].parse::<u8>().ok()?;if function>7 {return None;}
    Some(format!("{base}.{function}"))
}
fn extract_pci_bus_id(text:&str)->Option<String> {
    text.split(|c:char|!(c.is_ascii_hexdigit()||c==':'||c=='.'))
        .filter_map(|token|canonical_pci_token(token.trim_matches(':')))
        .find(|token|token.len()==12)
}
fn is_pci_bus_id(raw:&str)->bool {raw.len()==12 && canonical_pci_token(raw).is_some()}

/// Push a recent error message, keeping only last 5
fn push_recent_error(info: &mut GpuDmesgInfo, line: &str) {
    if info.recent_errors.len() >= 5 {
        info.recent_errors.remove(0);
    }
    // Truncate long lines
    let msg = if line.len() > 200 {
        let mut end=200;
        while !line.is_char_boundary(end) {end-=1;}
        format!("{}...", &line[..end])
    } else {
        line.to_string()
    };
    info.recent_errors.push(msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_pci_bus_id() {
        assert_eq!(
            extract_pci_bus_id("GPU 0000:c2:00.0: RmInitAdapter failed"),
            Some("0000:c2:00.0".to_string())
        );
        assert_eq!(
            extract_pci_bus_id("no pci id here"),
            None
        );
    }

    #[test]
    fn test_is_pci_bus_id() {
        assert!(is_pci_bus_id("0000:c2:00.0"));
        assert!(is_pci_bus_id("0000:01:00.0"));
        assert!(!is_pci_bus_id("not-a-pci-id"));
        assert!(!is_pci_bus_id("short"));
    }

    #[test]
    fn test_parse_rm_init_adapter() {
        let mut errors = DmesgGpuErrors::default();
        let input = r#"
[85836.136798] NVRM: GPU 0000:c2:00.0: RmInitAdapter failed! (0x62:0x65:2661)
[85836.140463] NVRM: GPU 0000:c2:00.0: rm_init_adapter failed, device minor number 1
[85843.457860] NVRM: GPU 0000:c2:00.0: RmInitAdapter failed! (0x62:0x65:2661)
"#;
        parse_lines(input, &mut errors);
        
        let gpu = errors.per_gpu.get("0000:c2:00.0").unwrap();
        assert_eq!(gpu.rm_init_adapter_failed, 3);
        assert!(gpu.is_hardware_failure);
        assert!(gpu.failure_summary.is_some());
    }

    #[test]
    fn test_parse_xid_error() {
        let line = "NVRM: Xid (PCI:0000:c2:00.0): 79, pid=1234, GPU has fallen off the bus";
        let result = extract_xid_error(line);
        assert_eq!(result, Some(("0000:c2:00.0".to_string(), 79)));
    }

    #[test]
    fn test_parse_fallen_off_bus() {
        let mut errors = DmesgGpuErrors::default();
        let input = "NVRM: GPU 0000:c2:00.0: GPU has fallen off the bus\n";
        parse_lines(input, &mut errors);
        
        assert!(errors.any_fallen_off_bus);
        let gpu = errors.per_gpu.get("0000:c2:00.0").unwrap();
        assert!(gpu.fallen_off_bus);
        assert!(gpu.is_hardware_failure);
    }

    #[test]
    fn failed_flagged_dmesg_uses_plain_dmesg_before_kern_log() {
        let plain = "NVRM: Xid (PCI:0000:23:00.0): 79, GPU has fallen off the bus";
        let kern = "NVRM: Xid (PCI:0000:41:00.0): 79, GPU has fallen off the bus";

        let errors = parse_first_available_source(
            || None,
            || Some(plain.to_string()),
            || Some(kern.to_string()),
        );

        assert_eq!(errors.source, Some(DmesgSource::PlainDmesg));
        assert!(errors.per_gpu.contains_key("0000:23:00.0"));
        assert!(!errors.per_gpu.contains_key("0000:41:00.0"));
    }

    // VER-HEALTH-004: sanitized observed NVIDIA prefix shapes; no tenant payload.
    #[test]
    fn health_definition_functionless_xid_prefixes_retain_observed_codes() {
        for code in [109, 119, 154] {
            let line = format!("NVRM: Xid (PCI:0000:61:00): {}, diagnostic omitted", code);
            assert_eq!(extract_xid_error(&line), Some(("0000:61:00".into(),code)), "{line}");
        }
    }

    #[test]
    fn health_definition_uppercase_xid_and_full_bdf_are_accepted() {
        assert_eq!(extract_xid_error("NVRM: XID (PCI:0000:AF:00.0): 154, omitted"),
            Some(("0000:af:00.0".into(),154)));
        let mut errors = DmesgGpuErrors::default();
        parse_lines("NVRM: XID (PCI:0000:AF:00.0): 154, omitted", &mut errors);
        assert_eq!(errors.per_gpu.get("0000:af:00.0").map(|x| x.xid_errors.as_slice()), Some(&[154][..]));
    }

    #[test]
    fn health_definition_invalid_pci_tokens_cannot_match_another_card() {
        for token in ["0000:61:00.8", "0000:61:20.0", "x0000:61:00.0", "00000:61:00.0", "0000:61:00.00"] {
            assert_eq!(extract_xid_error(&format!("NVRM: Xid (PCI:{}): 79, omitted",token)),None,"{token}");
        }
    }

    #[test]
    fn health_definition_xid_snapshot_reads_do_not_accumulate() {
        let read = || parse_first_available_source(
            || Some("NVRM: Xid (PCI:0000:61:00.0): 154, omitted\nNVRM: Xid (PCI:0000:61:00.0): 154, omitted".into()),
            || None, || None);
        assert_eq!(read().per_gpu["0000:61:00.0"].xid_errors, vec![154,154]);
        assert_eq!(read().per_gpu["0000:61:00.0"].xid_errors, vec![154,154]);
        let failed = parse_first_available_source(||None,||None,||None);
        let empty = parse_first_available_source(||Some(String::new()),||None,||None);
        assert!(!failed.read_succeeded);
        assert!(empty.read_succeeded);
        assert_eq!(empty.source,Some(DmesgSource::FlaggedDmesg));
    }

    #[test]
    fn health_definition_unicode_kernel_prefix_does_not_panic_or_hide_xid() {
        let actual=extract_xid_error("é kernel: NVRM: Xid (PCI:0000:61:00.0): 154, omitted");
        assert_eq!(actual,Some(("0000:61:00.0".into(),154)));
        assert_eq!(extract_pci_bus_id("é no valid pci token here"),None);
    }

    #[test]
    fn health_definition_long_unicode_log_payload_is_truncated_at_character_boundary() {
        let prefix="NVRM: Xid (PCI:0000:53:00.0): 154, ";
        let line=format!("{}{}éend",prefix,"x".repeat(199-prefix.len()));
        let mut errors=DmesgGpuErrors::default();parse_lines(&line,&mut errors);
        assert_eq!(errors.per_gpu["0000:53:00.0"].xid_errors,vec![154]);
        assert!(errors.per_gpu["0000:53:00.0"].recent_errors[0].ends_with("..."));
    }

}
