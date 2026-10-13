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
fn parse_lines(content: &str, errors: &mut DmesgGpuErrors) {
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
        if line.contains("Xid") || line.contains("xid") {
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

/// Extract XID error code and PCI bus ID from a line
fn extract_xid_error(line: &str) -> Option<(String, u32)> {
    let pci_id = extract_pci_id_from_nvrm(line)?;
    
    // Find "Xid" or "xid" followed by various formats
    let xid_pos = line.to_lowercase().find("xid")?;
    let after_xid = &line[xid_pos..];
    
    // Look for the error code after "): " or ": "
    // "Xid (PCI:0000:c2:00.0): 79, ..."
    if let Some(colon_pos) = after_xid.find("): ") {
        let code_str = &after_xid[colon_pos + 3..];
        let code_end = code_str.find(|c: char| !c.is_ascii_digit()).unwrap_or(code_str.len());
        if code_end > 0 {
            if let Ok(code) = code_str[..code_end].parse::<u32>() {
                return Some((pci_id, code));
            }
        }
    }
    
    None
}

/// Extract a PCI bus ID (xxxx:xx:xx.x format) from text
fn extract_pci_bus_id(text: &str) -> Option<String> {
    // Find pattern like "0000:c2:00.0"
    let bytes = text.as_bytes();
    let len = bytes.len();
    
    for i in 0..len {
        // Try to match xxxx:xx:xx.x (12 characters)
        if i + 12 <= len {
            let candidate = &text[i..i + 12];
            if is_pci_bus_id(candidate) {
                return Some(candidate.to_lowercase());
            }
        }
    }
    
    None
}

/// Check if a string looks like a PCI bus ID (xxxx:xx:xx.x)
fn is_pci_bus_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 12 {
        return false;
    }
    
    // xxxx:xx:xx.x
    bytes[0..4].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[4] == b':'
        && bytes[5..7].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[7] == b':'
        && bytes[8..10].iter().all(|b| b.is_ascii_hexdigit())
        && bytes[10] == b'.'
        && bytes[11].is_ascii_hexdigit()
}

/// Push a recent error message, keeping only last 5
fn push_recent_error(info: &mut GpuDmesgInfo, line: &str) {
    if info.recent_errors.len() >= 5 {
        info.recent_errors.remove(0);
    }
    // Truncate long lines
    let msg = if line.len() > 200 {
        format!("{}...", &line[..200])
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
}
