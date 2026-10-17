//! Metrics Collector
//!
//! Orchestrates collection of GPU metrics from multiple sources:
//! 1. PCIe bus scanning (always works, even during passthrough)
//! 2. NVML (for GPUs accessible to host)
//! 3. Direct register access (for VRAM/hotspot temps on consumer GPUs)
//! 4. VM guest agent (for GPUs passed through to VMs)
//!
//! CRITICAL SAFETY FEATURES:
//! - Never blocks on GPU operations (timeout-protected)
//! - Detects VM passthrough and collects metrics from inside VMs
//! - Gracefully degrades when GPUs are unavailable
//! - Always produces valid metrics output (even if just error states)

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use crate::state::{GpuArchitecture, GpuState};
use super::dmesg::parse_dmesg_gpu_errors;
use super::gpu::NvmlCollector;
use super::pci::{is_passthrough_driver, scan_nvidia_gpus, scan_qemu_passthrough_pci, PciGpuInfo};
use super::register::{get_gpu_info, is_dev_mem_accessible, read_gpu_temps};
use super::vm::VmMetricsCollector;
use super::{AllMetrics, GpuMetrics, MetricSource};

/// Main metrics collector that combines all sources
pub struct MetricsCollector {
    nvml_timeout: Duration,
    enable_register_access: bool,
    hardware_evidence_tracker: Arc<Mutex<HardwareEvidenceTracker>>,
    thermal_policy: super::thermal::ThermalPolicy,
    thermal_manager: Arc<super::thermal_worker::ThermalManager>,
    collection_interval: Duration,
}

impl MetricsCollector {
    pub fn new(nvml_timeout: Duration, enable_register_access: bool) -> Self {
        Self::with_thermal(nvml_timeout, enable_register_access, super::thermal::ThermalPolicy::default(), Duration::from_secs(10))
    }

    /// TRACE-THERMAL-002/006/007: one manager is retained across collection cycles.
    pub fn with_thermal(nvml_timeout: Duration, enable_register_access: bool, thermal_policy: super::thermal::ThermalPolicy, collection_interval: Duration) -> Self {
        let boot_log_baseline = parse_dmesg_gpu_errors();
        MetricsCollector {
            nvml_timeout,
            enable_register_access,
            hardware_evidence_tracker: Arc::new(Mutex::new(
                HardwareEvidenceTracker::from_boot_log(&boot_log_baseline),
            )),
            thermal_policy,
            thermal_manager: Arc::new(super::thermal_worker::ThermalManager::default()),
            collection_interval,
        }
    }

    /// Collect all metrics (async wrapper)
    pub async fn collect(&self) -> AllMetrics {
        // Run collection in blocking thread pool to avoid blocking async runtime
        let timeout = self.nvml_timeout;
        let register_access = self.enable_register_access;
        let hardware_evidence_tracker = Arc::clone(&self.hardware_evidence_tracker);
        let thermal_policy = self.thermal_policy.clone();
        let thermal_manager = Arc::clone(&self.thermal_manager);
        let collection_interval = self.collection_interval;
        
        tokio::task::spawn_blocking(move || {
            let collector = MetricsCollectorSync::new(
                timeout,
                register_access,
                hardware_evidence_tracker,
            );
            let mut metrics = collector.collect();
            thermal_policy.enrich(&mut metrics.gpus, &thermal_manager, collection_interval);
            metrics
        })
        .await
        .unwrap_or_else(|e| {
            warn!("Metrics collection task panicked: {}", e);
            AllMetrics {
                hostname: get_hostname(),
                timestamp: current_timestamp(),
                ..Default::default()
            }
        })
    }
}

/// Synchronous metrics collector (runs in blocking thread)
struct MetricsCollectorSync {
    nvml_collector: NvmlCollector,
    enable_register_access: bool,
    hardware_evidence_tracker: Arc<Mutex<HardwareEvidenceTracker>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BootHardwareEvidence {
    rm_init_adapter_failed: u32,
    terminal_xid_counts: HashMap<u32, u32>,
    fallen_off_bus: bool,
}

impl BootHardwareEvidence {
    fn monotonic_escalation_reason_from(&self, baseline: &Self) -> Option<String> {
        if self.fallen_off_bus && !baseline.fallen_off_bus {
            return Some("GPU fallen off bus".to_string());
        }

        let mut new_terminal_xids: Vec<u32> = self
            .terminal_xid_counts
            .iter()
            .filter_map(|(xid, count)| {
                (*count > baseline.terminal_xid_counts.get(xid).copied().unwrap_or(0))
                    .then_some(*xid)
            })
            .collect();
        new_terminal_xids.sort_unstable();
        if let Some(xid) = new_terminal_xids.first() {
            return Some(format!("Terminal XID {} detected", xid));
        }

        (self.rm_init_adapter_failed > baseline.rm_init_adapter_failed)
            .then(|| "RmInitAdapter failure detected".to_string())
    }
}

#[derive(Debug, Default)]
struct HardwareEvidenceTracker {
    last_successful_by_source:
        HashMap<super::dmesg::DmesgSource, HashMap<String, BootHardwareEvidence>>,
    // Original source-token counts must not collide after PCI projection: an
    // old functionless79 and a new full-BDF79 are distinct observed events.
    last_raw_by_source:
        HashMap<super::dmesg::DmesgSource, HashMap<String, BootHardwareEvidence>>,
    canonicalized_sources: HashSet<super::dmesg::DmesgSource>,
    latched_failure_reasons: HashMap<String, String>,
    unresolved_current_health: HashMap<(String,String),GpuState>,
}

impl HardwareEvidenceTracker {
    fn from_boot_log(dmesg_errors: &super::dmesg::DmesgGpuErrors) -> Self {
        let mut tracker = Self::default();
        tracker.initialize_baseline(dmesg_errors);
        tracker
    }

    fn initialize_baseline(&mut self, dmesg_errors: &super::dmesg::DmesgGpuErrors) {
        if !dmesg_errors.read_succeeded {
            return;
        }
        let Some(source) = dmesg_errors.source else {
            return;
        };
        if self.last_successful_by_source.contains_key(&source) {
            return;
        }

        let snapshot=terminal_evidence_snapshot(dmesg_errors);
        self.last_raw_by_source.entry(source).or_insert_with(||snapshot.clone());
        self.last_successful_by_source.insert(source,snapshot);
    }

    fn observe_terminal_evidence(
        &mut self,
        pci_bus_id: &str,
        dmesg_errors: &super::dmesg::DmesgGpuErrors,
    ) -> Option<String> {
        if !dmesg_errors.read_succeeded {
            return self.latched_failure_reasons.get(pci_bus_id).cloned();
        }
        let Some(source) = dmesg_errors.source else {
            return self.latched_failure_reasons.get(pci_bus_id).cloned();
        };

        // Canonicalized production snapshots have already compared ORIGINAL
        // source-token counts. Merged per-GPU presentation is not event authority.
        if self.canonicalized_sources.contains(&source) {
            return self.latched_failure_reasons.get(pci_bus_id).cloned();
        }

        if !self.last_successful_by_source.contains_key(&source) {
            self.initialize_baseline(dmesg_errors);
            return self.latched_failure_reasons.get(pci_bus_id).cloned();
        }

        let current = dmesg_errors
            .per_gpu
            .get(pci_bus_id)
            .and_then(terminal_boot_hardware_evidence);
        let source_evidence = self.last_successful_by_source.get_mut(&source).unwrap();
        let escalation_reason = current.as_ref().and_then(|evidence| {
            evidence.monotonic_escalation_reason_from(
                source_evidence
                    .get(pci_bus_id)
                    .unwrap_or(&BootHardwareEvidence::default()),
            )
        });

        if let Some(evidence) = current {
            source_evidence.insert(pci_bus_id.to_string(), evidence);
        } else {
            source_evidence.remove(pci_bus_id);
        }

        if let Some(reason) = escalation_reason {
            self.latched_failure_reasons
                .entry(pci_bus_id.to_string())
                .or_insert(reason);
        }

        self.latched_failure_reasons.get(pci_bus_id).cloned()
    }
}

fn terminal_evidence_snapshot(
    dmesg_errors: &super::dmesg::DmesgGpuErrors,
) -> HashMap<String, BootHardwareEvidence> {
    dmesg_errors
        .per_gpu
        .iter()
        .filter_map(|(pci_bus_id, info)| {
            terminal_boot_hardware_evidence(info)
                .map(|evidence| (pci_bus_id.clone(), evidence))
        })
        .collect()
}

fn terminal_boot_hardware_evidence(
    info: &super::dmesg::GpuDmesgInfo,
) -> Option<BootHardwareEvidence> {
    if !info.is_hardware_failure {
        return None;
    }

    let mut terminal_xid_counts = HashMap::new();
    for xid in &info.xid_errors {
        if super::dmesg::HARDWARE_FAILURE_XIDS.contains(xid) {
            *terminal_xid_counts.entry(*xid).or_insert(0) += 1;
        }
    }

    Some(BootHardwareEvidence {
        rm_init_adapter_failed: info.rm_init_adapter_failed,
        terminal_xid_counts,
        fallen_off_bus: info.fallen_off_bus,
    })
}

/// Reconcile a current NVML record with terminal hardware evidence.
///
/// Dmesg is boot-scoped history, so the tracker snapshots it before collection.
/// Any essential NVML failure is invalidated immediately; a changed terminal
/// evidence fingerprint or a current /proc failure upgrades even an otherwise
/// successful NVML record to a hardware failure.
/// Reconcile verified identities without manufacturing another physical GPU row.
/// A missing native BDF can join only by a unique verified /proc UUID; an
/// ambiguous/conflicting identity remains diagnostics-only, never index-joined.
fn reconcile_nvml_inventory(records:Vec<GpuMetrics>,inventory:&[PciGpuInfo])->(HashMap<String,GpuMetrics>,Vec<super::gpu::UnmatchedNvmlError>) {
    let mut bdf_counts=HashMap::<String,usize>::new();let mut uuid_counts=HashMap::<String,usize>::new();
    for gpu in &records {
        if !gpu.pci_bus_id.is_empty() {*bdf_counts.entry(gpu.pci_bus_id.clone()).or_default()+=1;}
        if !gpu.uuid.is_empty() {*uuid_counts.entry(gpu.uuid.clone()).or_default()+=1;}
    }
    let mut proposed=Vec::new();let mut target_counts=HashMap::<String,usize>::new();
    for gpu in records {
        let target=if !gpu.pci_bus_id.is_empty() {
            let matches:Vec<_>=inventory.iter().filter(|pci|pci.pci_bus_id==gpu.pci_bus_id).collect();
            let unique_native_uuid=gpu.uuid.is_empty() || uuid_counts.get(&gpu.uuid)==Some(&1);
            let proc_agrees=matches.len()==1 && matches[0].proc_health.as_ref().and_then(|health|health.uuid.as_deref()).filter(|uuid|!uuid.is_empty()).is_none_or(|uuid|gpu.uuid.is_empty() || uuid==gpu.uuid);
            let uuid_targets:Vec<_>=inventory.iter().filter(|pci|!gpu.uuid.is_empty() && pci.proc_health.as_ref().and_then(|health|health.uuid.as_deref())==Some(gpu.uuid.as_str())).collect();
            let no_cross_card_uuid=uuid_targets.is_empty() || (uuid_targets.len()==1 && uuid_targets[0].pci_bus_id==gpu.pci_bus_id);
            (bdf_counts.get(&gpu.pci_bus_id)==Some(&1) && matches.len()==1 && unique_native_uuid && proc_agrees && no_cross_card_uuid).then(||gpu.pci_bus_id.clone())
        } else if !gpu.uuid.is_empty() && uuid_counts.get(&gpu.uuid)==Some(&1) {
            let matches:Vec<_>=inventory.iter().filter(|pci|pci.proc_health.as_ref().and_then(|health|health.uuid.as_deref())==Some(gpu.uuid.as_str())).collect();
            (matches.len()==1 && inventory.iter().filter(|pci|pci.pci_bus_id==matches[0].pci_bus_id).count()==1).then(||matches[0].pci_bus_id.clone())
        } else {None};
        if let Some(target)=&target {*target_counts.entry(target.clone()).or_default()+=1;}
        proposed.push((target,gpu));
    }
    let mut joined=HashMap::new();let mut diagnostics=Vec::new();
    for (target,mut gpu) in proposed {
        if let Some(target)=target.filter(|target|target_counts.get(target)==Some(&1)) {
            gpu.pci_bus_id=target.clone();joined.insert(target,gpu);
        } else {
            if gpu.nvml_errors.is_empty() {gpu.nvml_errors.push(super::gpu::NvmlQueryError{query:"pci_info",error:"invalid_data",code:None});}
            for error in gpu.nvml_errors {
                diagnostics.push(super::gpu::UnmatchedNvmlError{index:gpu.index,uuid:gpu.uuid.clone(),pci_bus_id:gpu.pci_bus_id.clone(),name:gpu.name.clone(),driver_version:gpu.driver_version.clone(),error,recovery:gpu.recovery.clone()});
            }
        }
    }
    (joined,diagnostics)
}

/// A unique verified UUID may retain unresolved current evidence before BDF is
/// available. Ambiguous UUIDs and anonymous enumeration failures never bind it.
fn observe_unmatched_current_health(diagnostics:&[super::gpu::UnmatchedNvmlError],tracker:&mut HardwareEvidenceTracker) {
    let mut identities=HashMap::<&str,HashSet<(u32,&str)>>::new();
    for record in diagnostics {
        if !record.uuid.is_empty() {identities.entry(&record.uuid).or_default().insert((record.index,&record.pci_bus_id));}
    }
    let mut observed=HashSet::new();
    for record in diagnostics {
        if record.uuid.is_empty() || !record.pci_bus_id.is_empty() || identities.get(record.uuid.as_str()).is_none_or(|ids|ids.len()!=1) || !observed.insert(record.uuid.as_str()) {continue;}
        let Some(recovery)=&record.recovery else {continue;};
        let mut state=match recovery.status {
            super::gpu::RecoveryStatus::Valid if recovery.action.is_some_and(|code|code>0)=>GpuState::Error,
            super::gpu::RecoveryStatus::Valid|super::gpu::RecoveryStatus::Unsupported=>GpuState::Ok,
            _=>GpuState::Unavailable,
        };
        for evidence in diagnostics.iter().filter(|d|d.uuid==record.uuid && d.index==record.index) {
            let error=&evidence.error;
            let candidate=if matches!(error.code,Some(15|16)) || matches!(error.query,"name"|"temperature"|"memory") {GpuState::Error}
                else if matches!(error.error,"not_supported"|"function_not_found"|"symbol_unavailable"|"unsupported_slot") {GpuState::Ok}
                else {GpuState::Unavailable};
            if super::gpu::health_priority(candidate)>super::gpu::health_priority(state) {state=candidate;}
        }
        if recovery.status==super::gpu::RecoveryStatus::Unsupported {
            for ((uuid,_),prior) in &tracker.unresolved_current_health {
                if uuid==&record.uuid && super::gpu::health_priority(*prior)>super::gpu::health_priority(state) {state=*prior;}
            }
        } else {
            // Supported current evidence replaces prior current-state uncertainty;
            // the separate legacy terminal latch is intentionally untouched.
            tracker.unresolved_current_health.retain(|(uuid,_),_|uuid!=&record.uuid);
        }
        if matches!(state,GpuState::Error|GpuState::Unavailable) {tracker.unresolved_current_health.insert((record.uuid.clone(),String::new()),state);}
    }
}

/// Only unambiguous inventory identity may resolve a functionless kernel token.
fn resolve_xid_pci_id(token:&str,inventory:&[PciGpuInfo])->Option<String> {
    let canonical=super::dmesg::canonical_pci_token(token)?;
    let matches:Vec<_>=inventory.iter().filter(|gpu| {
        if canonical.len()==12 {gpu.pci_bus_id==canonical}
        else {gpu.pci_bus_id.get(..10)==Some(canonical.as_str())}
    }).collect();
    (matches.len()==1).then(||matches[0].pci_bus_id.clone())
}
/// Compare terminal events by original source token, then project diagnostics
/// only onto unambiguous PCI identity. Projection never supplies event counts.
fn canonicalize_dmesg_inventory(errors:&mut super::dmesg::DmesgGpuErrors,inventory:&[PciGpuInfo],tracker:&mut HardwareEvidenceTracker) {
    if errors.read_succeeded {
        if let Some(source)=errors.source {
            let current=terminal_evidence_snapshot(errors);
            let mut next=current.clone();
            if let Some(baseline)=tracker.last_raw_by_source.get(&source) {
                for (token,evidence) in &current {
                    if let Some(pci)=resolve_xid_pci_id(token,inventory) {
                        if let Some(reason)=evidence.monotonic_escalation_reason_from(baseline.get(token).unwrap_or(&BootHardwareEvidence::default())) {
                            tracker.latched_failure_reasons.entry(pci).or_insert(reason);
                        }
                    } else if let Some(previous)=baseline.get(token) {
                        // Leave unresolved changes pending until identity resolves.
                        next.insert(token.clone(),previous.clone());
                    } else {
                        next.remove(token);
                    }
                }
            }
            // Successful absence/rotation drops old tokens; failed reads never
            // replace history. First reads of a new source establish its baseline.
            tracker.last_raw_by_source.insert(source,next);
            tracker.canonicalized_sources.insert(source);
        }
    }
    let raw=std::mem::take(&mut errors.per_gpu);
    for (token,info) in raw {
        if let Some(pci)=resolve_xid_pci_id(&token,inventory) {
            let entry=errors.per_gpu.entry(pci).or_default();
            entry.rm_init_adapter_failed=entry.rm_init_adapter_failed.saturating_add(info.rm_init_adapter_failed);
            entry.xid_errors.extend(info.xid_errors);entry.fallen_off_bus|=info.fallen_off_bus;
            entry.is_hardware_failure|=info.is_hardware_failure;
            if entry.failure_summary.is_none() {entry.failure_summary=info.failure_summary;}
        }
    }
}

fn reconcile_nvml_hardware_evidence(mut gpu:GpuMetrics,pci_gpu:&PciGpuInfo,dmesg_errors:&super::dmesg::DmesgGpuErrors,evidence_tracker:&mut HardwareEvidenceTracker)->GpuMetrics {
    let hardware_failure_reason=active_hardware_failure_reason(pci_gpu,dmesg_errors,evidence_tracker);
    gpu.xid_source=dmesg_errors.source;gpu.xid_source_read_succeeded=dmesg_errors.read_succeeded;
    if let Some(info)=dmesg_errors.per_gpu.get(&pci_gpu.pci_bus_id) {
        gpu.dmesg_hardware_failure=info.is_hardware_failure;gpu.dmesg_rm_init_failed=info.rm_init_adapter_failed;
        gpu.fallen_off_bus=info.fallen_off_bus;gpu.xid_errors=info.xid_errors.clone();
    } else {gpu.xid_errors.clear();}
    // Current recovery/query state is keyed by both verified identities, never index.
    // Keep unresolved observations separate from the preexisting BDF terminal latch.
    let identity=(!gpu.uuid.is_empty() && !gpu.pci_bus_id.is_empty()).then(||(gpu.uuid.clone(),gpu.pci_bus_id.clone()));
    if gpu.recovery.is_some() && identity.is_none() && gpu.state!=GpuState::Error && gpu.state!=GpuState::HardwareFailure {gpu.state=GpuState::Unavailable;}
    if let Some(identity)=identity {
        let status=gpu.recovery.as_ref().map(|reading|reading.status);
        match status {
            Some(super::gpu::RecoveryStatus::Unsupported)=>{
                for key in [&identity,&(identity.0.clone(),String::new())] {
                    if let Some(prior)=evidence_tracker.unresolved_current_health.get(key) {
                        if super::gpu::health_priority(*prior)>super::gpu::health_priority(gpu.state) {gpu.state=*prior;}
                    }
                }
            }
            Some(_)=>{evidence_tracker.unresolved_current_health.retain(|(uuid,_),_|uuid!=&identity.0);}
            None=>{}
        }
        if gpu.recovery.is_some() && matches!(gpu.state,GpuState::Error|GpuState::Unavailable) {evidence_tracker.unresolved_current_health.insert(identity,gpu.state);}
    }
    if gpu.nvml_essential_query_failed {gpu.state=GpuState::Error;}
    if let Some(reason)=hardware_failure_reason {gpu.state=GpuState::HardwareFailure;gpu.hardware_failure_reason=Some(reason);}
    if !matches!(gpu.state,GpuState::Ok|GpuState::Warning) {clear_volatile_gpu_telemetry(&mut gpu);}
    gpu
}

fn enrich_pci_gpu_snapshot(gpu:&mut GpuMetrics,pci_gpu:&PciGpuInfo,dmesg_errors:&super::dmesg::DmesgGpuErrors) {
    gpu.bound_driver=pci_gpu.bound_driver.clone();
    gpu.device_id=pci_gpu.device_id;
    // Numeric observations belong to independently identified PCI rows even when
    // their current NVML handle, identity join, or operational telemetry fails.
    // Enrichment does not classify the row or associate anonymous NVML errors.
    if let Some(info)=dmesg_errors.per_gpu.get(&pci_gpu.pci_bus_id) {
        gpu.xid_errors=info.xid_errors.clone();
        gpu.dmesg_rm_init_failed=info.rm_init_adapter_failed;
        gpu.fallen_off_bus=info.fallen_off_bus;
    }
    gpu.xid_source=dmesg_errors.source;
    gpu.xid_source_read_succeeded=dmesg_errors.read_succeeded;
}

/// Check for a current /proc failure or new/latched terminal dmesg evidence.
#[cfg(test)]
fn has_hardware_failure_evidence(
    pci_gpu: &PciGpuInfo,
    dmesg_errors: &super::dmesg::DmesgGpuErrors,
    evidence_tracker: &mut HardwareEvidenceTracker,
) -> bool {
    active_hardware_failure_reason(pci_gpu, dmesg_errors, evidence_tracker).is_some()
}

fn active_hardware_failure_reason(
    pci_gpu: &PciGpuInfo,
    dmesg_errors: &super::dmesg::DmesgGpuErrors,
    evidence_tracker: &mut HardwareEvidenceTracker,
) -> Option<String> {
    let terminal_reason =
        evidence_tracker.observe_terminal_evidence(&pci_gpu.pci_bus_id, dmesg_errors);
    if terminal_reason.is_some() {
        return terminal_reason;
    }

    pci_gpu.proc_health.as_ref().and_then(|health| {
        (!health.is_healthy).then(|| {
            health
                .failure_reason
                .as_ref()
                .map(|reason| format!("/proc: {}", reason))
                .unwrap_or_else(|| "Current /proc GPU health failure".to_string())
        })
    })
}

fn clear_volatile_gpu_telemetry(gpu: &mut GpuMetrics) {
    gpu.gpu_temp = None;
    gpu.vram_temp = None;
    gpu.vram_thermal = None;
    gpu.hotspot_temp = None;
    gpu.vram_temp_approximated = false;
    gpu.hotspot_temp_approximated = false;
    gpu.power_usage = None;
    gpu.power_limit = None;
    gpu.power_max = None;
    gpu.sm_clock = None;
    gpu.mem_clock = None;
    gpu.gpu_util = None;
    gpu.mem_util = None;
    gpu.encoder_util = None;
    gpu.decoder_util = None;
    gpu.fb_used = None;
    gpu.fb_free = None;
    gpu.fb_total = None;
    gpu.fan_speed = None;
    gpu.throttle_reasons = Default::default();
    gpu.pcie_gen = None;
    gpu.pcie_gen_max = None;
    gpu.pcie_width = None;
    gpu.pcie_width_max = None;
    gpu.pcie_tx_throughput = None;
    gpu.pcie_rx_throughput = None;
    gpu.ecc_sbe = None;
    gpu.ecc_dbe = None;
    gpu.ecc_enabled = false;
    gpu.nvlink_active = 0;
    gpu.nvlink_total = 0;
    gpu.nvlink_tx_throughput = None;
    gpu.nvlink_rx_throughput = None;
    gpu.nvlink_errors = 0;
    gpu.compute_processes = 0;
    gpu.graphics_processes = 0;
    gpu.process_list.clear();
}

pub(super) fn count_usable_gpus(gpus: &[GpuMetrics]) -> u32 {
    gpus.iter()
        .filter(|gpu| {
            !gpu.nvml_essential_query_failed
                && matches!(
                    (gpu.source, gpu.state),
                    (MetricSource::Host | MetricSource::Vm, GpuState::Ok | GpuState::Warning)
                        | (MetricSource::Pci, GpuState::VmPassthrough)
                )
        })
        .count() as u32
}

fn gpus_available_from_enumeration(nvml_gpus: u32, vm_gpus: usize) -> bool {
    nvml_gpus > 0 || vm_gpus > 0
}

fn should_collect_register_telemetry(gpu: &GpuMetrics, register_available: bool) -> bool {
    register_available
        && !gpu.nvml_essential_query_failed
        && matches!(gpu.state, GpuState::Ok | GpuState::Warning)
        && gpu.architecture.use_register_access()
}

impl MetricsCollectorSync {
    fn new(
        nvml_timeout: Duration,
        enable_register_access: bool,
        hardware_evidence_tracker: Arc<Mutex<HardwareEvidenceTracker>>,
    ) -> Self {
        MetricsCollectorSync {
            nvml_collector: NvmlCollector::new(nvml_timeout),
            enable_register_access,
            hardware_evidence_tracker,
        }
    }

    fn collect(&self) -> AllMetrics {
        let hostname = get_hostname();
        let timestamp = current_timestamp();
        
        info!("Starting metrics collection");

        // Step 1: Scan PCIe bus for ALL NVIDIA GPUs (even those in VMs)
        // This NEVER blocks, even during passthrough
        let pci_gpus = scan_nvidia_gpus();
        let total_pci_gpus = pci_gpus.len() as u32;
        debug!("Found {} NVIDIA GPUs on PCIe bus", total_pci_gpus);

        // Build host GPU map (PCI bus ID -> index)
        let host_gpu_map: HashMap<String, u32> = pci_gpus
            .iter()
            .enumerate()
            .map(|(i, g)| (g.pci_bus_id.clone(), i as u32))
            .collect();

        // Step 2: Collect ALL NVML metrics in a SINGLE session (avoids FD leak!)
        // This is the ONLY Nvml::init() call per collection cycle
        let nvml_result = self.nvml_collector.collect_all();
        let nvml_gpus = nvml_result.gpus.len() as u32;
        let nvml_initialized = nvml_result.nvml_initialized;
        let mut nvml_unmatched_errors=nvml_result.unmatched_errors;
        let driver_version = nvml_result.gpus.first()
            .map(|g| g.driver_version.clone())
            .unwrap_or_else(|| "unknown".to_string());
        
        debug!("NVML returned {} GPU records, initialized={}", nvml_gpus, nvml_initialized);

        let (mut nvml_metrics,identity_diagnostics)=reconcile_nvml_inventory(nvml_result.gpus,&pci_gpus);
        nvml_unmatched_errors.extend(identity_diagnostics);

        // Step 4: Collect VM metrics for GPUs passed through to VMs
        // This also identifies which PCI addresses are confirmed passthrough via virsh dumpxml
        let vm_metrics: HashMap<u32, GpuMetrics> = VmMetricsCollector::collect_all(&host_gpu_map)
            .into_iter()
            .map(|g| (g.index, g))
            .collect();
        
        debug!("Collected metrics from {} VMs", vm_metrics.len());

        // Step 5: Build passthrough evidence set
        // We require POSITIVE EVIDENCE before classifying a GPU as VM passthrough.
        // Sources of evidence (any one is sufficient):
        //   1. Driver binding = "vfio-pci" or "pci-stub"
        //   2. QEMU process has this PCI address in command line
        //   3. virsh dumpxml shows this GPU passed to a running VM
        //   4. Not in /proc/driver/nvidia/gpus/ when nvidia driver IS loaded (unbound from nvidia)
        let mut confirmed_passthrough: HashSet<String> = HashSet::new();
        
        // Evidence source 1: vfio-pci/pci-stub driver binding (already in PciGpuInfo)
        for gpu in &pci_gpus {
            if let Some(ref driver) = gpu.bound_driver {
                if is_passthrough_driver(driver) {
                    confirmed_passthrough.insert(gpu.pci_bus_id.clone());
                    debug!("Passthrough evidence [driver={}]: {}", driver, gpu.pci_bus_id);
                }
            }
        }
        
        // Evidence source 2: QEMU process command lines
        let qemu_pci = scan_qemu_passthrough_pci();
        for pci_id in &qemu_pci {
            confirmed_passthrough.insert(pci_id.clone());
            debug!("Passthrough evidence [qemu]: {}", pci_id);
        }
        
        // Evidence source 3: virsh dumpxml (GPUs mapped to running VMs)
        // The VmMetricsCollector already identified these - any GPU that returned VM metrics
        // is confirmed passthrough. Additionally, check virsh directly for VMs without guest agent.
        let virsh_passthrough = VmMetricsCollector::get_all_passthrough_pci_ids();
        for pci_id in &virsh_passthrough {
            confirmed_passthrough.insert(pci_id.clone());
            debug!("Passthrough evidence [virsh]: {}", pci_id);
        }
        
        // Evidence source 4: Not in /proc but nvidia driver IS loaded
        // If nvidia driver is loaded and a GPU does NOT appear in /proc/driver/nvidia/gpus/,
        // the GPU was intentionally unbound from nvidia (passthrough preparation)
        let nvidia_driver_loaded = pci_gpus.iter().any(|g| g.proc_health.is_some());
        if nvidia_driver_loaded {
            for gpu in &pci_gpus {
                if gpu.proc_health.is_none() && gpu.bound_driver.as_deref() != Some("nvidia") {
                    // GPU not in /proc AND not bound to nvidia = likely passthrough
                    // (but only if we don't already have it as a different state)
                    debug!("Passthrough evidence [not-in-proc, no nvidia driver]: {}", gpu.pci_bus_id);
                    confirmed_passthrough.insert(gpu.pci_bus_id.clone());
                }
            }
        }
        
        if !confirmed_passthrough.is_empty() {
            info!("Confirmed passthrough GPUs: {:?}", confirmed_passthrough);
        }

        // Step 6: Parse dmesg for hardware failure signals (RmInitAdapter, XID, fallen off bus)
        let mut dmesg_errors = parse_dmesg_gpu_errors();
        {
            let mut tracker=self.hardware_evidence_tracker.lock().unwrap_or_else(|p|p.into_inner());
            canonicalize_dmesg_inventory(&mut dmesg_errors,&pci_gpus,&mut tracker);
            observe_unmatched_current_health(&nvml_unmatched_errors,&mut tracker);
        }
        if !dmesg_errors.per_gpu.is_empty() {
            info!("Dmesg GPU errors found for {} GPUs", dmesg_errors.per_gpu.len());
        }

        // Step 7: Build final GPU list, merging ALL evidence
        // Decision tree per GPU:
        //   1. In NVML → OK (host GPU, normal metrics)
        //   2. In VM metrics → OK (from VM, got data from guest agent)
        //   3. Hardware failure signals → HardwareFailure (dmesg/proc evidence)
        //   4. In confirmed_passthrough set → VmPassthrough (positive evidence)
        //   5. Everything else → Error/Unavailable (NEVER assume passthrough without evidence)
        let mut gpus = Vec::with_capacity(pci_gpus.len());
        let register_available = self.enable_register_access && is_dev_mem_accessible();

        for (pci_index, pci_gpu) in pci_gpus.iter().enumerate() {
            let pci_index = pci_index as u32;
            let has_passthrough_evidence = confirmed_passthrough.contains(&pci_gpu.pci_bus_id);
            
            let mut gpu = if let Some(mut nvml_gpu) = nvml_metrics.remove(&pci_gpu.pci_bus_id) {
                // 1. GPU is accessible via NVML (on host)
                nvml_gpu.index = pci_index;

                // A persistent NVML handle can continue returning stale values after a GPU
                // falls off the bus. Only override current NVML data when this collection
                // also failed an essential query; historical dmesg alone is not sufficient.
                nvml_gpu = {
                    let mut evidence_tracker = self
                        .hardware_evidence_tracker
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    reconcile_nvml_hardware_evidence(
                        nvml_gpu,
                        pci_gpu,
                        &dmesg_errors,
                        &mut evidence_tracker,
                    )
                };
                
                // Try direct register access for VRAM/hotspot temps
                if should_collect_register_telemetry(&nvml_gpu, register_available) {
                    self.add_register_temps(&mut nvml_gpu, pci_gpu);
                }
                
                nvml_gpu
            } else if let Some(vm_gpu) = vm_metrics.get(&pci_index) {
                // 2. GPU is in VM - got metrics from guest agent
                debug!("GPU {} using VM metrics (VRAM: {:?}, Hotspot: {:?})", 
                    pci_index, vm_gpu.vram_temp, vm_gpu.hotspot_temp);
                vm_gpu.clone()
            } else if let Some(reason) = {
                let mut evidence_tracker = self
                    .hardware_evidence_tracker
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                active_hardware_failure_reason(
                    pci_gpu,
                    &dmesg_errors,
                    &mut evidence_tracker,
                )
            } {
                // 3. Confirmed hardware failure (dmesg RmInitAdapter, broken VBIOS, etc.)
                // This takes priority over passthrough evidence because a defective GPU
                // should never be silently classified as "in a VM"
                warn!("GPU {} ({}) - CONFIRMED HARDWARE FAILURE: {}",
                    pci_index, pci_gpu.pci_bus_id, reason);
                self.create_hardware_failure_gpu(pci_index, pci_gpu, &driver_version, &reason, &dmesg_errors)
            } else if has_passthrough_evidence {
                // 4. Positive evidence of VM passthrough
                // At least one source confirmed: vfio-pci/pci-stub driver, QEMU process, 
                // virsh dumpxml, or not in /proc when nvidia is loaded
                debug!("GPU {} ({}) has passthrough evidence - confirmed VM passthrough",
                    pci_index, pci_gpu.pci_bus_id);
                self.create_passthrough_gpu(pci_index, pci_gpu, &driver_version)
            } else if !nvml_initialized {
                // 5. NVML failed to initialize entirely (driver error)
                warn!("GPU {} ({}) - NVML failed, marking as Error (driver issue)", 
                    pci_index, pci_gpu.pci_bus_id);
                self.create_error_gpu(pci_index, pci_gpu, &driver_version)
            } else {
                // 6. NVML initialized but GPU not found, NO passthrough evidence
                // This is NOT passthrough - it's an error or unavailable state
                warn!("GPU {} ({}) not in NVML, no passthrough evidence (driver={:?}, in_proc={}) - marking as Error",
                    pci_index, pci_gpu.pci_bus_id, pci_gpu.bound_driver, pci_gpu.proc_health.is_some());
                self.create_error_gpu(pci_index, pci_gpu, &driver_version)
            };
            
            enrich_pci_gpu_snapshot(&mut gpu,pci_gpu,&dmesg_errors);
            gpus.push(gpu);
        }
        // Step 6: Collect system metrics
        let apt_upgradable = count_apt_upgradable();
        AllMetrics {
            hostname,
            driver_version,
            total_pci_gpus,
            nvml_gpus,
            gpus_available: gpus_available_from_enumeration(nvml_gpus, vm_metrics.len()),
            gpus,
            apt_upgradable,
            timestamp,
            nvml_unmatched_errors,
        }
    }

    /// Add VRAM and hotspot temps via direct register access
    fn add_register_temps(&self, gpu: &mut GpuMetrics, pci_gpu: &PciGpuInfo) {
        // Check if this GPU supports register access
        let gpu_info = match get_gpu_info(pci_gpu.device_id) {
            Some(info) => info,
            None => {
                debug!(
                    "GPU {} (device_id=0x{:04X}) not in supported list for register access",
                    pci_gpu.pci_bus_id, pci_gpu.device_id
                );
                return;
            }
        };

        // Get BAR0 address
        let bar0_addr = match pci_gpu.bar0_addr {
            Some(addr) => addr,
            None => {
                debug!("No BAR0 address for GPU {}", pci_gpu.pci_bus_id);
                return;
            }
        };

        // Read temperatures
        let temps = read_gpu_temps(bar0_addr, gpu_info.vram_offset);
        
        if let Some(vram) = temps.vram_temp {
            gpu.vram_temp = Some(vram);
            gpu.vram_temp_approximated = false;
            gpu.vram_thermal = Some(super::thermal::ThermalMetadata { backend: super::thermal::ThermalBackend::Legacy, status: super::thermal::ThermalStatus::Valid });
            debug!("GPU {}: VRAM temp = {}°C (via register)", gpu.index, vram);
        }
        
        if let Some(hotspot) = temps.hotspot_temp {
            gpu.hotspot_temp = Some(hotspot);
            gpu.hotspot_temp_approximated = false;
            debug!("GPU {}: Hotspot temp = {}°C (via register)", gpu.index, hotspot);
        }
    }

    /// Create metrics for a GPU with confirmed hardware failure
    fn create_hardware_failure_gpu(
        &self,
        index: u32,
        pci_gpu: &PciGpuInfo,
        driver_version: &str,
        failure_reason: &str,
        dmesg_errors: &super::dmesg::DmesgGpuErrors,
    ) -> GpuMetrics {
        // Try to get UUID and name from /proc if available
        let (name, uuid) = if let Some(ref health) = pci_gpu.proc_health {
            (
                health.model.clone().unwrap_or_else(|| format!("GPU-{:04X}-HW-FAIL", pci_gpu.device_id)),
                health.uuid.clone().unwrap_or_else(|| "HW-FAILURE".to_string()),
            )
        } else {
            (
                format!("GPU-{:04X}-HW-FAIL", pci_gpu.device_id),
                "HW-FAILURE".to_string(),
            )
        };
        
        let mut gpu = GpuMetrics {
            index,
            name,
            uuid,
            pci_bus_id: pci_gpu.pci_bus_id.clone(),
            driver_version: driver_version.to_string(),
            state: GpuState::HardwareFailure,
            architecture: GpuArchitecture::Unknown,
            source: MetricSource::Pci,
            hardware_failure_reason: Some(failure_reason.to_string()),
            dmesg_hardware_failure: true,
            bound_driver: pci_gpu.bound_driver.clone(),
            ..Default::default()
        };
        
        // Copy dmesg error details
        if let Some(dmesg_info) = dmesg_errors.per_gpu.get(&pci_gpu.pci_bus_id) {
            gpu.dmesg_rm_init_failed = dmesg_info.rm_init_adapter_failed;
            gpu.fallen_off_bus = dmesg_info.fallen_off_bus;
            gpu.xid_errors = dmesg_info.xid_errors.clone();
        }
        
        gpu
    }

    /// Create metrics for a GPU in VM passthrough
    fn create_passthrough_gpu(&self, index: u32, pci_gpu: &PciGpuInfo, driver_version: &str) -> GpuMetrics {
        debug!(
            "GPU {} ({}) not in NVML - marking as VM passthrough (driver={:?})",
            index, pci_gpu.pci_bus_id, pci_gpu.bound_driver
        );
        
        GpuMetrics {
            index,
            name: format!("GPU-{:04X}-VM", pci_gpu.device_id),
            uuid: "VM-PASSTHROUGH".to_string(),
            pci_bus_id: pci_gpu.pci_bus_id.clone(),
            driver_version: driver_version.to_string(),
            state: GpuState::VmPassthrough,
            architecture: GpuArchitecture::Unknown,
            source: MetricSource::Pci,
            bound_driver: pci_gpu.bound_driver.clone(),
            // All metrics unavailable for passthrough GPUs
            gpu_temp: None,
            vram_temp: None,
            hotspot_temp: None,
            power_usage: None,
            sm_clock: None,
            mem_clock: None,
            gpu_util: None,
            mem_util: None,
            fb_used: None,
            fb_free: None,
            fan_speed: None,
            aer_errors: 0,
            ..Default::default()
        }
    }
    
    /// Create metrics for a GPU that's inaccessible due to driver error
    /// This is different from VM passthrough - the GPU is present but the driver failed
    fn create_error_gpu(&self, index: u32, pci_gpu: &PciGpuInfo, driver_version: &str) -> GpuMetrics {
        warn!(
            "GPU {} ({}) - NVML failed to initialize, marking as Error (driver={:?})",
            index, pci_gpu.pci_bus_id, pci_gpu.bound_driver
        );
        
        GpuMetrics {
            index,
            name: format!("GPU-{:04X}-ERROR", pci_gpu.device_id),
            uuid: "DRIVER-ERROR".to_string(),
            pci_bus_id: pci_gpu.pci_bus_id.clone(),
            driver_version: driver_version.to_string(),
            state: GpuState::Error,  // Error state, not VM Passthrough
            architecture: GpuArchitecture::Unknown,
            source: MetricSource::Pci,
            bound_driver: pci_gpu.bound_driver.clone(),
            // All metrics unavailable due to driver error
            gpu_temp: None,
            vram_temp: None,
            hotspot_temp: None,
            power_usage: None,
            sm_clock: None,
            mem_clock: None,
            gpu_util: None,
            mem_util: None,
            fb_used: None,
            fb_free: None,
            fan_speed: None,
            aer_errors: 0,
            ..Default::default()
        }
    }
}

/// Get hostname
fn get_hostname() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Get current Unix timestamp
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Count APT upgradable packages (Linux only)
fn count_apt_upgradable() -> Option<u32> {
    // Try reading from cache file first (faster)
    if let Ok(content) = std::fs::read_to_string("/var/log/package-count.txt") {
        for line in content.lines() {
            if line.starts_with("Upgradable packages:") {
                if let Some(count_str) = line.split(':').nth(1) {
                    if let Ok(count) = count_str.trim().parse::<u32>() {
                        return Some(count);
                    }
                }
            }
        }
    }
    
    // Fallback: run apt list (slower, avoid in hot path)
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        
        let output = Command::new("apt")
            .args(["list", "--upgradable"])
            .output()
            .ok()?;
        
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Count lines that contain "upgradable from"
            let count = stdout
                .lines()
                .filter(|line| line.contains("upgradable from"))
                .count() as u32;
            return Some(count);
        }
    }
    
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::dmesg::{DmesgGpuErrors, DmesgSource, GpuDmesgInfo};
    use crate::metrics::pci::ProcGpuHealth;
    use crate::metrics::prometheus::PrometheusFormatter;
    use crate::state::ThrottleReasons;

    fn pci_gpu(pci_bus_id: &str) -> PciGpuInfo {
        PciGpuInfo {
            pci_bus_id: pci_bus_id.to_string(),
            vendor_id: 0x10de,
            device_id: 0x2b85,
            in_nvml: true,
            bar0_addr: Some(0x1000),
            bound_driver: Some("nvidia".to_string()),
            proc_health: None,
        }
    }

    fn terminal_dmesg(pci_bus_id: &str) -> DmesgGpuErrors {
        let mut errors = DmesgGpuErrors::default();
        errors.read_succeeded = true;
        errors.source = Some(DmesgSource::FlaggedDmesg);
        errors.any_fallen_off_bus = true;
        errors.per_gpu.insert(
            pci_bus_id.to_string(),
            GpuDmesgInfo {
                rm_init_adapter_failed: 1,
                xid_errors: vec![79],
                fallen_off_bus: true,
                is_hardware_failure: true,
                ..Default::default()
            },
        );
        errors
    }

    fn successful_empty_dmesg() -> DmesgGpuErrors {
        DmesgGpuErrors {
            read_succeeded: true,
            source: Some(DmesgSource::FlaggedDmesg),
            ..Default::default()
        }
    }

    fn from_dmesg_source(
        mut errors: DmesgGpuErrors,
        source: DmesgSource,
    ) -> DmesgGpuErrors {
        errors.source = Some(source);
        errors
    }

    fn stale_nvml_gpu(pci_bus_id: &str) -> GpuMetrics {
        GpuMetrics {
            index: 1,
            name: "NVIDIA GeForce RTX 5090".to_string(),
            uuid: "GPU-KEEP-THIS-UUID".to_string(),
            pci_bus_id: pci_bus_id.to_string(),
            driver_version: "590.48.01".to_string(),
            state: GpuState::Warning,
            architecture: GpuArchitecture::Consumer,
            source: MetricSource::Host,
            nvml_essential_query_failed: true,
            gpu_temp: Some(58),
            vram_temp: Some(127),
            hotspot_temp: Some(91),
            power_usage: Some(530.377),
            gpu_util: Some(98),
            fb_total: Some(32_000),
            throttle_reasons: ThrottleReasons {
                hw_slowdown: true,
                hw_thermal_slowdown: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn hardware_failure_sample(gpu: GpuMetrics) -> String {
        PrometheusFormatter::format(&AllMetrics {
            hostname: "runpodccc95".to_string(),
            gpus: vec![gpu],
            ..Default::default()
        })
        .lines()
        .find(|line| line.starts_with("DCXP_GPU_HW_FAILURE{"))
        .unwrap()
        .to_string()
    }

    #[test]
    fn test_hostname() {
        let hostname = get_hostname();
        assert!(!hostname.is_empty());
    }

    #[test]
    fn test_timestamp() {
        let ts = current_timestamp();
        assert!(ts > 0);
    }

    #[test]
    fn new_terminal_dmesg_overrides_failed_nvml_record() {
        let pci_bus_id = "0000:23:00.0";
        let mut evidence_tracker = HardwareEvidenceTracker::default();

        let mut healthy_baseline = stale_nvml_gpu(pci_bus_id);
        healthy_baseline.state = GpuState::Ok;
        healthy_baseline.nvml_essential_query_failed = false;
        let healthy_baseline = reconcile_nvml_hardware_evidence(
            healthy_baseline,
            &pci_gpu(pci_bus_id),
            &successful_empty_dmesg(),
            &mut evidence_tracker,
        );
        assert_eq!(healthy_baseline.state, GpuState::Ok);

        let gpu = reconcile_nvml_hardware_evidence(
            stale_nvml_gpu(pci_bus_id),
            &pci_gpu(pci_bus_id),
            &terminal_dmesg(pci_bus_id),
            &mut evidence_tracker,
        );

        assert_eq!(gpu.state, GpuState::HardwareFailure);
        assert_eq!(gpu.name, "NVIDIA GeForce RTX 5090");
        assert_eq!(gpu.uuid, "GPU-KEEP-THIS-UUID");
        assert_eq!(gpu.pci_bus_id, pci_bus_id);
        assert_eq!(gpu.driver_version, "590.48.01");
        assert_eq!(gpu.gpu_temp, None);
        assert_eq!(gpu.vram_temp, None);
        assert_eq!(gpu.hotspot_temp, None);
        assert_eq!(gpu.power_usage, None);
        assert_eq!(gpu.gpu_util, None);
        assert_eq!(gpu.fb_total, None);
        assert!(!gpu.throttle_reasons.hw_slowdown);
        assert!(!gpu.throttle_reasons.hw_thermal_slowdown);
        assert_eq!(gpu.xid_errors, vec![79]);
        assert!(gpu.fallen_off_bus);
        assert!(gpu.dmesg_hardware_failure);
    }

    #[test]
    fn new_terminal_evidence_overrides_successful_nvml_after_healthy_baseline() {
        let pci_bus_id = "0000:23:00.0";
        let pci_gpu = pci_gpu(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::default();

        let mut healthy_baseline = stale_nvml_gpu(pci_bus_id);
        healthy_baseline.state = GpuState::Ok;
        healthy_baseline.nvml_essential_query_failed = false;
        let healthy_baseline = reconcile_nvml_hardware_evidence(
            healthy_baseline,
            &pci_gpu,
            &successful_empty_dmesg(),
            &mut evidence_tracker,
        );
        assert_eq!(healthy_baseline.state, GpuState::Ok);

        let mut stale_success = stale_nvml_gpu(pci_bus_id);
        stale_success.state = GpuState::Ok;
        stale_success.nvml_essential_query_failed = false;
        let failed = reconcile_nvml_hardware_evidence(
            stale_success,
            &pci_gpu,
            &terminal_dmesg(pci_bus_id),
            &mut evidence_tracker,
        );

        assert_eq!(failed.state, GpuState::HardwareFailure);
        assert_eq!(failed.name, "NVIDIA GeForce RTX 5090");
        assert_eq!(failed.uuid, "GPU-KEEP-THIS-UUID");
        assert_eq!(failed.gpu_temp, None);
        assert_eq!(failed.vram_temp, None);
        assert_eq!(failed.power_usage, None);
        assert!(!failed.throttle_reasons.hw_thermal_slowdown);
        assert_eq!(failed.xid_errors, vec![79]);
        assert!(failed.fallen_off_bus);
    }

    #[test]
    fn historical_boot_evidence_is_baselined_before_first_transient_scrape_after_restart() {
        let pci_bus_id = "0000:23:00.0";
        let mut never_observed_tracker = HardwareEvidenceTracker::default();

        let transient_failure = reconcile_nvml_hardware_evidence(
            stale_nvml_gpu(pci_bus_id),
            &pci_gpu(pci_bus_id),
            &terminal_dmesg(pci_bus_id),
            &mut never_observed_tracker,
        );

        assert_eq!(transient_failure.state, GpuState::Error);
        assert_eq!(transient_failure.gpu_temp, None);
        assert_eq!(transient_failure.vram_temp, None);
        assert!(!transient_failure.throttle_reasons.hw_thermal_slowdown);
        assert_eq!(transient_failure.xid_errors, vec![79]);
        assert!(transient_failure.fallen_off_bus);
    }

    #[test]
    fn pci_only_historical_terminal_evidence_after_restart_is_generic_error() {
        let pci_bus_id = "0000:23:00.0";
        let historical = terminal_dmesg(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::from_boot_log(&historical);

        assert!(!has_hardware_failure_evidence(
            &pci_gpu(pci_bus_id),
            &historical,
            &mut evidence_tracker,
        ));
    }

    #[test]
    fn pci_only_new_terminal_evidence_is_immediate_hardware_failure() {
        let pci_bus_id = "0000:23:00.0";
        let mut evidence_tracker =
            HardwareEvidenceTracker::from_boot_log(&successful_empty_dmesg());

        assert!(has_hardware_failure_evidence(
            &pci_gpu(pci_bus_id),
            &terminal_dmesg(pci_bus_id),
            &mut evidence_tracker,
        ));
    }

    #[test]
    fn pci_only_current_proc_failure_remains_immediate() {
        let pci_bus_id = "0000:23:00.0";
        let historical = terminal_dmesg(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::from_boot_log(&historical);
        let mut failed_pci = pci_gpu(pci_bus_id);
        failed_pci.proc_health = Some(ProcGpuHealth {
            model: Some("NVIDIA GeForce RTX 5090".to_string()),
            uuid: Some("GPU-KEEP-THIS-UUID".to_string()),
            video_bios: None,
            firmware: None,
            is_healthy: false,
            failure_reason: Some("GPU firmware unavailable".to_string()),
        });

        assert!(has_hardware_failure_evidence(
            &failed_pci,
            &historical,
            &mut evidence_tracker,
        ));
    }

    #[test]
    fn new_non_terminal_xid_does_not_change_terminal_evidence_fingerprint() {
        let pci_bus_id = "0000:23:00.0";
        let pci_gpu = pci_gpu(pci_bus_id);
        let baseline = terminal_dmesg(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::default();

        let mut healthy = stale_nvml_gpu(pci_bus_id);
        healthy.state = GpuState::Ok;
        healthy.nvml_essential_query_failed = false;
        let healthy = reconcile_nvml_hardware_evidence(
            healthy,
            &pci_gpu,
            &baseline,
            &mut evidence_tracker,
        );
        assert_eq!(healthy.state, GpuState::Ok);

        let mut changed_log = baseline;
        changed_log
            .per_gpu
            .get_mut(pci_bus_id)
            .unwrap()
            .xid_errors
            .push(31);
        let mut healthy_after_non_terminal_xid = stale_nvml_gpu(pci_bus_id);
        healthy_after_non_terminal_xid.state = GpuState::Ok;
        healthy_after_non_terminal_xid.nvml_essential_query_failed = false;

        let healthy_after_non_terminal_xid = reconcile_nvml_hardware_evidence(
            healthy_after_non_terminal_xid,
            &pci_gpu,
            &changed_log,
            &mut evidence_tracker,
        );

        assert_eq!(healthy_after_non_terminal_xid.state, GpuState::Ok);
        assert_eq!(healthy_after_non_terminal_xid.gpu_temp, Some(58));
    }

    #[test]
    fn boot_log_rotation_does_not_retrigger_terminal_evidence() {
        let pci_bus_id = "0000:23:00.0";
        let pci_gpu = pci_gpu(pci_bus_id);
        let mut baseline = terminal_dmesg(pci_bus_id);
        let baseline_info = baseline.per_gpu.get_mut(pci_bus_id).unwrap();
        baseline_info.rm_init_adapter_failed = 2;
        baseline_info.xid_errors = vec![79, 79, 48];
        let mut evidence_tracker = HardwareEvidenceTracker::default();

        let mut healthy = stale_nvml_gpu(pci_bus_id);
        healthy.state = GpuState::Ok;
        healthy.nvml_essential_query_failed = false;
        let healthy = reconcile_nvml_hardware_evidence(
            healthy,
            &pci_gpu,
            &baseline,
            &mut evidence_tracker,
        );
        assert_eq!(healthy.state, GpuState::Ok);

        let mut rotated_log = terminal_dmesg(pci_bus_id);
        let rotated_info = rotated_log.per_gpu.get_mut(pci_bus_id).unwrap();
        rotated_info.rm_init_adapter_failed = 1;
        rotated_info.xid_errors = vec![48, 79];
        let mut healthy_after_rotation = stale_nvml_gpu(pci_bus_id);
        healthy_after_rotation.state = GpuState::Ok;
        healthy_after_rotation.nvml_essential_query_failed = false;

        let healthy_after_rotation = reconcile_nvml_hardware_evidence(
            healthy_after_rotation,
            &pci_gpu,
            &rotated_log,
            &mut evidence_tracker,
        );

        assert_eq!(healthy_after_rotation.state, GpuState::Ok);
        assert_eq!(healthy_after_rotation.gpu_temp, Some(58));

        let mut new_same_type_event = rotated_log;
        new_same_type_event
            .per_gpu
            .get_mut(pci_bus_id)
            .unwrap()
            .xid_errors
            .push(79);
        let mut stale_success = stale_nvml_gpu(pci_bus_id);
        stale_success.state = GpuState::Ok;
        stale_success.nvml_essential_query_failed = false;

        let failed = reconcile_nvml_hardware_evidence(
            stale_success,
            &pci_gpu,
            &new_same_type_event,
            &mut evidence_tracker,
        );
        assert_eq!(failed.state, GpuState::HardwareFailure);
        assert_eq!(failed.gpu_temp, None);

        let mut unchanged_success = stale_nvml_gpu(pci_bus_id);
        unchanged_success.state = GpuState::Ok;
        unchanged_success.nvml_essential_query_failed = false;
        let still_failed = reconcile_nvml_hardware_evidence(
            unchanged_success,
            &pci_gpu,
            &new_same_type_event,
            &mut evidence_tracker,
        );
        assert_eq!(still_failed.state, GpuState::HardwareFailure);
        assert_eq!(still_failed.gpu_temp, None);
    }

    #[test]
    fn unreadable_dmesg_does_not_rebase_last_successful_terminal_evidence() {
        let pci_bus_id = "0000:23:00.0";
        let pci_gpu = pci_gpu(pci_bus_id);
        let historical = terminal_dmesg(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::from_boot_log(&historical);

        let mut healthy_during_failed_read = stale_nvml_gpu(pci_bus_id);
        healthy_during_failed_read.state = GpuState::Ok;
        healthy_during_failed_read.nvml_essential_query_failed = false;
        let healthy_during_failed_read = reconcile_nvml_hardware_evidence(
            healthy_during_failed_read,
            &pci_gpu,
            &DmesgGpuErrors::default(),
            &mut evidence_tracker,
        );
        assert_eq!(healthy_during_failed_read.state, GpuState::Ok);

        let mut healthy_after_read_recovers = stale_nvml_gpu(pci_bus_id);
        healthy_after_read_recovers.state = GpuState::Ok;
        healthy_after_read_recovers.nvml_essential_query_failed = false;
        let healthy_after_read_recovers = reconcile_nvml_hardware_evidence(
            healthy_after_read_recovers,
            &pci_gpu,
            &historical,
            &mut evidence_tracker,
        );

        assert_eq!(healthy_after_read_recovers.state, GpuState::Ok);
        assert_eq!(healthy_after_read_recovers.gpu_temp, Some(58));
    }

    #[test]
    fn successful_dmesg_disappearance_rebases_and_same_type_event_fires_again() {
        let pci_bus_id = "0000:23:00.0";
        let pci_gpu = pci_gpu(pci_bus_id);
        let historical = terminal_dmesg(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::default();

        let mut healthy_with_history = stale_nvml_gpu(pci_bus_id);
        healthy_with_history.state = GpuState::Ok;
        healthy_with_history.nvml_essential_query_failed = false;
        let healthy_with_history = reconcile_nvml_hardware_evidence(
            healthy_with_history,
            &pci_gpu,
            &historical,
            &mut evidence_tracker,
        );
        assert_eq!(healthy_with_history.state, GpuState::Ok);

        let mut healthy_after_disappearance = stale_nvml_gpu(pci_bus_id);
        healthy_after_disappearance.state = GpuState::Ok;
        healthy_after_disappearance.nvml_essential_query_failed = false;
        let healthy_after_disappearance = reconcile_nvml_hardware_evidence(
            healthy_after_disappearance,
            &pci_gpu,
            &successful_empty_dmesg(),
            &mut evidence_tracker,
        );
        assert_eq!(healthy_after_disappearance.state, GpuState::Ok);

        let mut stale_success = stale_nvml_gpu(pci_bus_id);
        stale_success.state = GpuState::Ok;
        stale_success.nvml_essential_query_failed = false;
        let failed_again = reconcile_nvml_hardware_evidence(
            stale_success,
            &pci_gpu,
            &terminal_dmesg(pci_bus_id),
            &mut evidence_tracker,
        );

        assert_eq!(failed_again.state, GpuState::HardwareFailure);
        assert_eq!(failed_again.gpu_temp, None);
    }

    #[test]
    fn latched_failure_reason_and_metric_identity_survive_rotation_and_new_occurrences() {
        let pci_bus_id = "0000:23:00.0";
        let pci_gpu = pci_gpu(pci_bus_id);
        let mut evidence_tracker =
            HardwareEvidenceTracker::from_boot_log(&successful_empty_dmesg());

        let first_event = terminal_dmesg(pci_bus_id);
        let first_failure = reconcile_nvml_hardware_evidence(
            stale_nvml_gpu(pci_bus_id),
            &pci_gpu,
            &first_event,
            &mut evidence_tracker,
        );
        assert_eq!(first_failure.state, GpuState::HardwareFailure);

        let after_rotation = reconcile_nvml_hardware_evidence(
            stale_nvml_gpu(pci_bus_id),
            &pci_gpu,
            &successful_empty_dmesg(),
            &mut evidence_tracker,
        );
        assert_eq!(after_rotation.state, GpuState::HardwareFailure);

        let mut appended_occurrence = first_event;
        appended_occurrence
            .per_gpu
            .get_mut(pci_bus_id)
            .unwrap()
            .xid_errors
            .push(79);
        let after_appended_occurrence = reconcile_nvml_hardware_evidence(
            stale_nvml_gpu(pci_bus_id),
            &pci_gpu,
            &appended_occurrence,
            &mut evidence_tracker,
        );
        assert_eq!(after_appended_occurrence.state, GpuState::HardwareFailure);

        let first_reason = first_failure.hardware_failure_reason.clone();
        assert_eq!(after_rotation.hardware_failure_reason, first_reason);
        assert_eq!(after_appended_occurrence.hardware_failure_reason, first_reason);
        assert_eq!(
            hardware_failure_sample(after_rotation),
            hardware_failure_sample(first_failure.clone()),
        );
        assert_eq!(
            hardware_failure_sample(after_appended_occurrence),
            hardware_failure_sample(first_failure),
        );
    }

    #[test]
    fn switching_to_empty_kern_log_and_back_to_unchanged_dmesg_does_not_latch() {
        let pci_bus_id = "0000:23:00.0";
        let pci_gpu = pci_gpu(pci_bus_id);
        let dmesg_history = terminal_dmesg(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::from_boot_log(&dmesg_history);

        let empty_kern_log = from_dmesg_source(
            successful_empty_dmesg(),
            DmesgSource::KernLog,
        );
        let mut healthy_on_kern_log = stale_nvml_gpu(pci_bus_id);
        healthy_on_kern_log.state = GpuState::Ok;
        healthy_on_kern_log.nvml_essential_query_failed = false;
        let healthy_on_kern_log = reconcile_nvml_hardware_evidence(
            healthy_on_kern_log,
            &pci_gpu,
            &empty_kern_log,
            &mut evidence_tracker,
        );
        assert_eq!(healthy_on_kern_log.state, GpuState::Ok);

        let mut healthy_back_on_dmesg = stale_nvml_gpu(pci_bus_id);
        healthy_back_on_dmesg.state = GpuState::Ok;
        healthy_back_on_dmesg.nvml_essential_query_failed = false;
        let healthy_back_on_dmesg = reconcile_nvml_hardware_evidence(
            healthy_back_on_dmesg,
            &pci_gpu,
            &dmesg_history,
            &mut evidence_tracker,
        );

        assert_eq!(healthy_back_on_dmesg.state, GpuState::Ok);
        assert_eq!(healthy_back_on_dmesg.gpu_temp, Some(58));
    }

    #[test]
    fn larger_alternate_log_corpus_establishes_its_own_baseline_without_latching() {
        let pci_bus_id = "0000:23:00.0";
        let pci_gpu = pci_gpu(pci_bus_id);
        let dmesg_history = terminal_dmesg(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::from_boot_log(&dmesg_history);

        let mut larger_kern_log = from_dmesg_source(
            terminal_dmesg(pci_bus_id),
            DmesgSource::KernLog,
        );
        let larger_info = larger_kern_log.per_gpu.get_mut(pci_bus_id).unwrap();
        larger_info.rm_init_adapter_failed = 12;
        larger_info.xid_errors = vec![79; 12];
        let mut healthy = stale_nvml_gpu(pci_bus_id);
        healthy.state = GpuState::Ok;
        healthy.nvml_essential_query_failed = false;

        let healthy = reconcile_nvml_hardware_evidence(
            healthy,
            &pci_gpu,
            &larger_kern_log,
            &mut evidence_tracker,
        );

        assert_eq!(healthy.state, GpuState::Ok);
        assert_eq!(healthy.gpu_temp, Some(58));
    }

    #[test]
    fn same_source_terminal_increment_latches_hardware_failure() {
        let pci_bus_id = "0000:23:00.0";
        let pci_gpu = pci_gpu(pci_bus_id);
        let baseline = terminal_dmesg(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::from_boot_log(&baseline);
        let mut incremented = baseline;
        incremented
            .per_gpu
            .get_mut(pci_bus_id)
            .unwrap()
            .xid_errors
            .push(79);
        let mut stale_success = stale_nvml_gpu(pci_bus_id);
        stale_success.state = GpuState::Ok;
        stale_success.nvml_essential_query_failed = false;

        let failed = reconcile_nvml_hardware_evidence(
            stale_success,
            &pci_gpu,
            &incremented,
            &mut evidence_tracker,
        );

        assert_eq!(failed.state, GpuState::HardwareFailure);
        assert_eq!(failed.gpu_temp, None);
    }

    #[test]
    fn healthy_current_nvml_data_is_not_overridden_by_historical_dmesg() {
        let pci_bus_id = "0000:23:00.0";
        let mut gpu = stale_nvml_gpu(pci_bus_id);
        gpu.state = GpuState::Ok;
        gpu.nvml_essential_query_failed = false;
        let mut evidence_tracker = HardwareEvidenceTracker::default();

        let gpu = reconcile_nvml_hardware_evidence(
            gpu,
            &pci_gpu(pci_bus_id),
            &terminal_dmesg(pci_bus_id),
            &mut evidence_tracker,
        );

        assert_eq!(gpu.state, GpuState::Ok);
        assert_eq!(gpu.gpu_temp, Some(58));
        assert!(gpu.throttle_reasons.hw_thermal_slowdown);
    }

    #[test]
    fn essential_nvml_failure_without_hardware_evidence_is_invalidated() {
        let pci_bus_id = "0000:41:00.0";
        let mut evidence_tracker = HardwareEvidenceTracker::default();
        let gpu = reconcile_nvml_hardware_evidence(
            stale_nvml_gpu(pci_bus_id),
            &pci_gpu(pci_bus_id),
            &successful_empty_dmesg(),
            &mut evidence_tracker,
        );

        assert_eq!(gpu.state, GpuState::Error);
        assert_eq!(gpu.name, "NVIDIA GeForce RTX 5090");
        assert_eq!(gpu.uuid, "GPU-KEEP-THIS-UUID");
        assert_eq!(gpu.gpu_temp, None);
        assert_eq!(gpu.vram_temp, None);
        assert_eq!(gpu.power_usage, None);
        assert_eq!(gpu.gpu_util, None);
        assert!(!gpu.throttle_reasons.hw_slowdown);
        assert!(!gpu.throttle_reasons.hw_thermal_slowdown);
        assert!(!should_collect_register_telemetry(&gpu, true));
        assert_eq!(count_usable_gpus(&[gpu]), 0);
    }

    #[test]
    fn baseline_boot_evidence_does_not_repromote_a_transient_query_failure() {
        let pci_bus_id = "0000:23:00.0";
        let pci_gpu = pci_gpu(pci_bus_id);
        let boot_evidence = terminal_dmesg(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::default();

        let mut healthy = stale_nvml_gpu(pci_bus_id);
        healthy.state = GpuState::Ok;
        healthy.nvml_essential_query_failed = false;
        let healthy = reconcile_nvml_hardware_evidence(
            healthy,
            &pci_gpu,
            &boot_evidence,
            &mut evidence_tracker,
        );
        assert_eq!(healthy.state, GpuState::Ok);

        let transient_failure = reconcile_nvml_hardware_evidence(
            stale_nvml_gpu(pci_bus_id),
            &pci_gpu,
            &boot_evidence,
            &mut evidence_tracker,
        );
        assert_eq!(transient_failure.state, GpuState::Error);
        assert_eq!(transient_failure.gpu_temp, None);
        assert!(!transient_failure.throttle_reasons.hw_thermal_slowdown);

        let mut recovered = stale_nvml_gpu(pci_bus_id);
        recovered.state = GpuState::Ok;
        recovered.nvml_essential_query_failed = false;
        let recovered = reconcile_nvml_hardware_evidence(
            recovered,
            &pci_gpu,
            &boot_evidence,
            &mut evidence_tracker,
        );
        assert_eq!(recovered.state, GpuState::Ok);
        assert_eq!(recovered.gpu_temp, Some(58));
    }

    #[test]
    fn current_proc_failure_remains_immediate_after_boot_evidence_was_acknowledged() {
        let pci_bus_id = "0000:23:00.0";
        let boot_evidence = terminal_dmesg(pci_bus_id);
        let mut evidence_tracker = HardwareEvidenceTracker::default();

        let mut healthy = stale_nvml_gpu(pci_bus_id);
        healthy.state = GpuState::Ok;
        healthy.nvml_essential_query_failed = false;
        reconcile_nvml_hardware_evidence(
            healthy,
            &pci_gpu(pci_bus_id),
            &boot_evidence,
            &mut evidence_tracker,
        );

        let mut current_failure_pci = pci_gpu(pci_bus_id);
        current_failure_pci.proc_health = Some(ProcGpuHealth {
            model: Some("NVIDIA GeForce RTX 5090".to_string()),
            uuid: Some("GPU-KEEP-THIS-UUID".to_string()),
            video_bios: None,
            firmware: None,
            is_healthy: false,
            failure_reason: Some("GPU firmware unavailable".to_string()),
        });

        let failed = reconcile_nvml_hardware_evidence(
            stale_nvml_gpu(pci_bus_id),
            &current_failure_pci,
            &boot_evidence,
            &mut evidence_tracker,
        );
        assert_eq!(failed.state, GpuState::HardwareFailure);
        assert_eq!(failed.gpu_temp, None);
    }

    #[test]
    fn trustworthy_nvml_count_excludes_failed_or_untrustworthy_records() {
        let mut healthy = stale_nvml_gpu("0000:01:00.0");
        healthy.state = GpuState::Ok;
        healthy.nvml_essential_query_failed = false;

        let mut evidence_tracker = HardwareEvidenceTracker::default();
        let failed = reconcile_nvml_hardware_evidence(
            stale_nvml_gpu("0000:23:00.0"),
            &pci_gpu("0000:23:00.0"),
            &terminal_dmesg("0000:23:00.0"),
            &mut evidence_tracker,
        );

        let mut partial = stale_nvml_gpu("0000:41:00.0");
        partial.state = GpuState::Warning;
        partial.nvml_essential_query_failed = true;

        assert_eq!(count_usable_gpus(&[healthy, failed, partial]), 1);
    }

    #[test]
    fn constructed_nvml_record_keeps_global_availability_when_telemetry_is_invalid() {
        let pci_bus_id = "0000:41:00.0";
        let mut evidence_tracker = HardwareEvidenceTracker::default();
        let invalid = reconcile_nvml_hardware_evidence(
            stale_nvml_gpu(pci_bus_id),
            &pci_gpu(pci_bus_id),
            &successful_empty_dmesg(),
            &mut evidence_tracker,
        );

        assert_eq!(count_usable_gpus(&[invalid]), 0);
        assert!(gpus_available_from_enumeration(1, 0));
        assert!(!gpus_available_from_enumeration(0, 0));
    }

    #[test]
    fn usable_count_accounts_for_valid_host_vm_and_confirmed_passthrough_gpus() {
        let healthy_host = GpuMetrics {
            state: GpuState::Ok,
            source: MetricSource::Host,
            ..Default::default()
        };
        let valid_vm = GpuMetrics {
            state: GpuState::Ok,
            source: MetricSource::Vm,
            ..Default::default()
        };
        let confirmed_passthrough = GpuMetrics {
            state: GpuState::VmPassthrough,
            source: MetricSource::Pci,
            ..Default::default()
        };
        let invalid_host = GpuMetrics {
            state: GpuState::Warning,
            source: MetricSource::Host,
            nvml_essential_query_failed: true,
            ..Default::default()
        };
        let hardware_failure = GpuMetrics {
            state: GpuState::HardwareFailure,
            source: MetricSource::Host,
            ..Default::default()
        };

        assert_eq!(
            count_usable_gpus(&[
                healthy_host,
                valid_vm,
                confirmed_passthrough,
                invalid_host,
                hardware_failure,
            ]),
            3
        );
    }

    // VER-HEALTH-003/005: actual reconciliation -> actual formatter, no invented output.
    #[test]
    fn health_definition_nominal_reconciliation_retains_historical_154_without_fault() {
        let bdf = "0000:61:00.0";
        let mut history = successful_empty_dmesg();
        history.per_gpu.insert(bdf.into(), GpuDmesgInfo { xid_errors: vec![154], ..Default::default() });
        let mut tracker = HardwareEvidenceTracker::from_boot_log(&history);
        let mut input = stale_nvml_gpu(bdf);
        input.state = GpuState::Ok;
        input.nvml_essential_query_failed = false;
        let actual = reconcile_nvml_hardware_evidence(input,&pci_gpu(bdf),&history,&mut tracker);
        assert_eq!(actual.state.as_u8(),1);
        assert_eq!(actual.xid_errors,vec![154]);
        assert_eq!(count_usable_gpus(&[actual.clone()]),1);
        let output = PrometheusFormatter::format(&AllMetrics { hostname: "fixture-host".into(), gpus:vec![actual], ..Default::default() });
        assert!(output.lines().any(|line|line.starts_with("DCXP_GPU_XID_CODE{") && line.contains("code=\"154\"") && line.ends_with(" 1")));
    }

    #[test]
    fn health_definition_terminal_latch_survives_nominal_and_missing_history() {
        let bdf="0000:61:00.0";
        let mut tracker=HardwareEvidenceTracker::from_boot_log(&successful_empty_dmesg());
        let mut gpu=stale_nvml_gpu(bdf); gpu.nvml_essential_query_failed=false; gpu.state=GpuState::Ok;
        let fault=reconcile_nvml_hardware_evidence(gpu.clone(),&pci_gpu(bdf),&terminal_dmesg(bdf),&mut tracker);
        assert_eq!(fault.state.as_u8(),5);
        let later=reconcile_nvml_hardware_evidence(gpu,&pci_gpu(bdf),&DmesgGpuErrors::default(),&mut tracker);
        assert_eq!(later.state.as_u8(),5);
        assert_eq!(count_usable_gpus(&[later.clone()]),0);
        assert!(!should_collect_register_telemetry(&later,true));
        assert_eq!(later.uuid,"GPU-KEEP-THIS-UUID");
        assert!(later.power_usage.is_none());
    }

    #[test]
    fn health_definition_eight_records_keep_failed_identity_and_seven_usable() {
        let mut tracker=HardwareEvidenceTracker::from_boot_log(&successful_empty_dmesg());
        let cards:Vec<_>=(0..8).map(|index| {
            let bdf=format!("0000:{:02x}:00.0",0x50+index);
            let mut gpu=stale_nvml_gpu(&bdf); gpu.index=index; gpu.uuid=format!("GPU-SYNTHETIC-{index}");
            gpu.nvml_essential_query_failed=index==3; gpu.state=GpuState::Ok;
            reconcile_nvml_hardware_evidence(gpu,&pci_gpu(&bdf),&successful_empty_dmesg(),&mut tracker)
        }).collect();
        assert_eq!(count_usable_gpus(&cards),7);
        assert_eq!(cards[3].uuid,"GPU-SYNTHETIC-3");
        assert_eq!(cards[3].pci_bus_id,"0000:53:00.0");
        assert_eq!(cards[3].state.as_u8(),2);
        assert!(!should_collect_register_telemetry(&cards[3],true));
        let output=PrometheusFormatter::format(&AllMetrics {hostname:"fixture-host".into(),gpus:cards,..Default::default()});
        assert_eq!(output.lines().filter(|line|line.starts_with("GPU_ERROR_STATE{")).count(),8);
        assert!(!output.lines().any(|line|line.starts_with("DCGM_FI_DEV_POWER_USAGE{") && line.contains("GPU-SYNTHETIC-3")));
    }

// VER-HEALTH-001/003/005: actual adapter -> reconciliation -> formatter stateful test.
    fn health_definition_recovery_gpu(code: Result<u32,nvml_wrapper::error::NvmlError>) -> GpuMetrics {
        for key in ["DCXP_FIXTURE_TAG","DCXP_FIXTURE_BAD_FIELD","DCXP_FIXTURE_NESTED_ERROR","DCXP_FIXTURE_UNTOUCHED"] {std::env::remove_var(key);}
        match code {
            Ok(n)=>{std::env::set_var("DCXP_FIXTURE_RECOVERY",n.to_string());std::env::remove_var("DCXP_FIXTURE_QUERY");}
            Err(nvml_wrapper::error::NvmlError::NotSupported)=>{std::env::set_var("DCXP_FIXTURE_QUERY","recovery_action");std::env::set_var("DCXP_FIXTURE_ERROR","3");}
            Err(nvml_wrapper::error::NvmlError::Unknown)=>{std::env::set_var("DCXP_FIXTURE_QUERY","recovery_action");std::env::set_var("DCXP_FIXTURE_ERROR","999");}
            _=>panic!("fixture accepts only explicit cases"),
        }
        let context=super::super::gpu::NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_fixture_path().as_os_str()).unwrap();
        super::super::gpu::collect_metrics_from_nvml(&context).gpus.remove(3)
    }
    #[test]
    fn health_definition_future_snapshot_recovery_lifecycle_and_restart() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        use nvml_wrapper::error::NvmlError;
        let pci=pci_gpu("0000:53:00.0");let empty=successful_empty_dmesg();
        let mut tracker=HardwareEvidenceTracker::from_boot_log(&empty);
        for (code,state) in [(Ok(0),1),(Ok(1),2),(Ok(0),1),(Ok(1),2),(Err(NvmlError::Unknown),0),(Ok(0),1),(Ok(1),2),(Err(NvmlError::NotSupported),2),(Ok(0),1)] {
            let actual=reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(code),&pci,&empty,&mut tracker);
            assert_eq!(actual.state.as_u8(),state);
            assert_eq!(count_usable_gpus(&[actual.clone()]),if state==1 {1}else{0});
            if state!=1 {assert!(!should_collect_register_telemetry(&actual,true));assert!(actual.power_usage.is_none());}
            assert_eq!(actual.uuid,"GPU-SYNTHETIC-3");
        }
        let mut restarted=HardwareEvidenceTracker::from_boot_log(&empty);
        assert_eq!(reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Err(NvmlError::NotSupported)),&pci,&empty,&mut restarted).state.as_u8(),1);
    }
    #[test]
    fn health_definition_future_terminal_latch_and_startup_history_are_separate_from_recovery() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        let pci=pci_gpu("0000:53:00.0");let terminal=terminal_dmesg(&pci.pci_bus_id);let empty=successful_empty_dmesg();
        let mut tracker=HardwareEvidenceTracker::from_boot_log(&empty);
        for history in [&terminal,&empty,&DmesgGpuErrors::default()] {
            let actual=reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,history,&mut tracker);
            assert_eq!(actual.state.as_u8(),5);assert_eq!(count_usable_gpus(&[actual]),0);
        }
        let mut restarted=HardwareEvidenceTracker::from_boot_log(&terminal);
        let actual=reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&terminal,&mut restarted);
        assert_eq!(actual.state.as_u8(),1);assert_eq!(actual.xid_errors,vec![79]);
    }
    #[test]
    fn health_definition_future_pci_identity_resolution_refuses_ambiguity_and_ignores_index_order() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        let first=pci_gpu("0000:53:00.0");let second=pci_gpu("0000:62:00.0");
        assert_eq!(resolve_xid_pci_id("0000:53:00",&[second.clone(),first.clone()]),Some(first.pci_bus_id.clone()));
        assert_eq!(resolve_xid_pci_id("0000:53:00",&[first.clone(),pci_gpu("0000:53:00.1")]),None);
        assert_eq!(resolve_xid_pci_id("0000:53:00.0",&[second]),None);
        assert_eq!(resolve_xid_pci_id("",&[first]),None);
    }
    #[test]
    fn health_definition_future_source_read_status_distinguishes_unavailable_empty_and_truncated() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        let pci=pci_gpu("0000:53:00.0");
        let mut many=successful_empty_dmesg();many.per_gpu.insert(pci.pci_bus_id.clone(),GpuDmesgInfo {xid_errors:(1000..1065).collect(),..Default::default()});
        for (history,source,status) in [(DmesgGpuErrors::default(),"none","unavailable"),(successful_empty_dmesg(),"flagged_dmesg","valid"),(many,"flagged_dmesg","truncated")] {
            let mut tracker=HardwareEvidenceTracker::from_boot_log(&history);
            let gpu=reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&history,&mut tracker);
            let output=PrometheusFormatter::format(&AllMetrics {hostname:"fixture-host".into(),gpus:vec![gpu],..Default::default()});
            assert!(output.lines().any(|l|l.starts_with("DCXP_GPU_XID_SOURCE_STATUS{") && l.contains(&format!("source=\"{source}\"")) && l.contains(&format!("status=\"{status}\"")) && l.ends_with(" 1")));
        }
    }
    #[test]
    fn health_definition_future_functionless_startup_and_new_terminal_baselines_share_canonical_identity() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        let pci=pci_gpu("0000:53:00.0");
        let mut parsed=DmesgGpuErrors {source:Some(DmesgSource::FlaggedDmesg),..Default::default()};
        super::super::dmesg::parse_lines("NVRM: Xid (PCI:0000:53:00): 79, omitted",&mut parsed);
        assert!(parsed.per_gpu.contains_key("0000:53:00"),"parser must retain functionless identity before inventory exists");
        let mut startup=HardwareEvidenceTracker::from_boot_log(&parsed);
        let mut historical=parsed.clone();
        canonicalize_dmesg_inventory(&mut historical,&[pci.clone()],&mut startup);
        let actual=reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&historical,&mut startup);
        assert_eq!(actual.state.as_u8(),1,"canonicalization cannot invent a postbaseline terminal event");assert_eq!(actual.xid_errors,vec![79]);
        let output=PrometheusFormatter::format(&AllMetrics {gpus:vec![actual],..Default::default()});
        assert!(output.lines().any(|l|l.starts_with("DCXP_GPU_XID_CODE{") && l.contains("code=\"79\"") && l.ends_with(" 1")));
        let mut fresh=HardwareEvidenceTracker::from_boot_log(&successful_empty_dmesg());
        let mut new_event=parsed.clone();canonicalize_dmesg_inventory(&mut new_event,&[pci.clone()],&mut fresh);
        assert_eq!(reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&new_event,&mut fresh).state.as_u8(),5);
        let mut ambiguous=HardwareEvidenceTracker::from_boot_log(&parsed);
        let mut unmatched=parsed;canonicalize_dmesg_inventory(&mut unmatched,&[pci.clone(),pci_gpu("0000:53:00.1")],&mut ambiguous);
        let actual=reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&unmatched,&mut ambiguous);
        assert_eq!(actual.state.as_u8(),1);assert!(actual.xid_errors.is_empty());
        // Resolving an earlier ambiguity cannot turn startup history into a new event.
        let mut resolved_later=DmesgGpuErrors {source:Some(DmesgSource::FlaggedDmesg),..Default::default()};
        super::super::dmesg::parse_lines("NVRM: Xid (PCI:0000:53:00): 79, omitted",&mut resolved_later);
        canonicalize_dmesg_inventory(&mut resolved_later,&[pci.clone()],&mut ambiguous);
        let later=reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&resolved_later,&mut ambiguous);
        assert_eq!(later.state.as_u8(),1,"newly resolvable startup evidence is still historical");
    }
    #[test]
    fn health_definition_future_unresolved_recovery_cache_uses_verified_identity_not_index() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        use nvml_wrapper::error::NvmlError;
        let pci=pci_gpu("0000:53:00.0");let empty=successful_empty_dmesg();
        let mut tracker=HardwareEvidenceTracker::from_boot_log(&empty);
        let reset=health_definition_recovery_gpu(Ok(1));
        assert_eq!(reconcile_nvml_hardware_evidence(reset.clone(),&pci,&empty,&mut tracker).state.as_u8(),2);
        let mut reordered=health_definition_recovery_gpu(Err(NvmlError::NotSupported));reordered.index=7;
        assert_eq!(reconcile_nvml_hardware_evidence(reordered,&pci,&empty,&mut tracker).state.as_u8(),2,"same verified UUID/BDF retains unresolved fault after enumeration changes");
        let mut replacement=health_definition_recovery_gpu(Err(NvmlError::NotSupported));replacement.uuid="GPU-SYNTHETIC-REPLACEMENT".into();
        assert_eq!(reconcile_nvml_hardware_evidence(replacement,&pci,&empty,&mut tracker).state.as_u8(),1,"new verified card at same BDF cannot inherit another UUID's recovery cache");
        let mut missing=health_definition_recovery_gpu(Err(NvmlError::NotSupported));missing.uuid.clear();missing.pci_bus_id.clear();missing.index=reset.index;
        let actual=reconcile_nvml_hardware_evidence(missing,&pci,&empty,&mut tracker);
        assert_eq!(actual.state.as_u8(),0,"missing identity cannot borrow another card's fault or nominal clearance by index");
        let unchanged=reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Err(NvmlError::NotSupported)),&pci,&empty,&mut tracker);
        assert_eq!(unchanged.state.as_u8(),2,"unidentified sample cannot clear the original unresolved card");
    }
    #[test]
    fn health_definition_future_device_handle_failures_are_unmatched_diagnostics_not_index_joined() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        for (native,error) in [(15,"gpu_lost"),(16,"reset_required"),(999,"unknown")] {
            for key in ["DCXP_FIXTURE_TAG","DCXP_FIXTURE_BAD_FIELD","DCXP_FIXTURE_NESTED_ERROR","DCXP_FIXTURE_UNTOUCHED"] {std::env::remove_var(key);}
            std::env::set_var("DCXP_FIXTURE_RECOVERY","0");std::env::set_var("DCXP_FIXTURE_QUERY","device_by_index");std::env::set_var("DCXP_FIXTURE_ERROR",native.to_string());
            let context=super::super::gpu::NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_fixture_path().as_os_str()).unwrap();
            let snapshot=super::super::gpu::collect_metrics_from_nvml(&context);
            assert_eq!(snapshot.gpus.len(),7);assert_eq!(snapshot.unmatched_errors.len(),1);
            assert!(snapshot.gpus.iter().all(|gpu|gpu.uuid!="GPU-SYNTHETIC-3"));
            let collector=MetricsCollectorSync::new(Duration::from_millis(1),false,Arc::new(Mutex::new(HardwareEvidenceTracker::default())));
            // Independently supplied PCI fallback is constructed by the actual fallback adapter.
            let fallback=collector.create_error_gpu(99,&pci_gpu("0000:53:00.0"),"615.71.09");
            assert_eq!(fallback.state.as_u8(),2);assert!(!should_collect_register_telemetry(&fallback,true));
            let mut gpus=snapshot.gpus;gpus.push(fallback);
            let metrics=AllMetrics {hostname:"fixture-host".into(),total_pci_gpus:8,nvml_gpus:7,gpus,nvml_unmatched_errors:snapshot.unmatched_errors,..Default::default()};
            let output=PrometheusFormatter::format(&metrics);
            assert_eq!(output.lines().filter(|line|line.starts_with("GPU_ERROR_STATE{")).count(),8);
            assert!(!output.lines().any(|line|line.starts_with("GPU_ERROR_STATE{") && line.contains("UUID=\"\"")));
            assert!(output.contains("DCXP_GPU_COUNT{type=\"nvml\",Hostname=\"fixture-host\"} 7"));
            assert!(output.contains("DCXP_GPU_COUNT{type=\"usable\",Hostname=\"fixture-host\"} 7"));
            let diagnostic:Vec<_>=output.lines().filter(|line|line.starts_with("DCXP_GPU_NVML_ERROR{") && line.contains("query=\"device_by_index\"")).collect();
            assert_eq!(diagnostic.len(),1);assert!(diagnostic[0].contains("gpu=\"3\""));assert!(diagnostic[0].contains("UUID=\"\""));assert!(diagnostic[0].contains("pci_bus_id=\"\""));
            assert!(diagnostic[0].contains(&format!("error=\"{error}\"")));assert!(diagnostic[0].contains(&format!("code=\"{native}\"")));
        }
    }

    #[test]
    fn health_definition_partial_identity_joins_unique_proc_uuid_or_exports_diagnostics_only() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        std::env::set_var("DCXP_FIXTURE_RECOVERY","1");std::env::set_var("DCXP_FIXTURE_QUERY","pci_info");std::env::set_var("DCXP_FIXTURE_ERROR","15");
        for key in ["DCXP_FIXTURE_TAG","DCXP_FIXTURE_BAD_FIELD","DCXP_FIXTURE_NESTED_ERROR","DCXP_FIXTURE_UNTOUCHED","DCXP_FIXTURE_MODEL"] {std::env::remove_var(key);}
        let context=super::super::gpu::NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_fixture_path().as_os_str()).unwrap();
        let native=super::super::gpu::collect_metrics_from_nvml(&context);
        assert_eq!(native.gpus.len(),8);assert!(native.gpus[3].pci_bus_id.is_empty());
        let mut inventory:Vec<_>=(0..8).map(|i|pci_gpu(&format!("0000:{:02x}:00.0",0x50+i))).collect();
        inventory[3].proc_health=Some(ProcGpuHealth {uuid:Some("GPU-SYNTHETIC-3".into()),is_healthy:true,model:None,video_bios:None,firmware:None,failure_reason:None});
        let (joined,diagnostics)=reconcile_nvml_inventory(native.gpus.clone(),&inventory);
        assert_eq!(joined.len(),8);assert!(diagnostics.is_empty());assert_eq!(joined["0000:53:00.0"].uuid,"GPU-SYNTHETIC-3");assert_eq!(joined["0000:53:00.0"].state.as_u8(),2);
        inventory[3].proc_health=None;
        let (joined,diagnostics)=reconcile_nvml_inventory(native.gpus,&inventory);
        assert_eq!(joined.len(),7);assert!(!diagnostics.is_empty());
        let collector=MetricsCollectorSync::new(Duration::from_millis(1),false,Arc::new(Mutex::new(HardwareEvidenceTracker::default())));
        let mut gpus:Vec<_>=joined.into_values().collect();gpus.push(collector.create_error_gpu(99,&inventory[3],"615.71.09"));
        let output=PrometheusFormatter::format(&AllMetrics {hostname:"fixture-host".into(),gpus,nvml_gpus:8,total_pci_gpus:8,nvml_unmatched_errors:diagnostics,..Default::default()});
        assert_eq!(output.lines().filter(|l|l.starts_with("GPU_ERROR_STATE{")).count(),8);
        assert!(!output.lines().any(|l|l.starts_with("GPU_ERROR_STATE{") && l.contains("GPU-SYNTHETIC-3")));
        assert!(output.lines().any(|l|l.starts_with("DCXP_GPU_NVML_ERROR{") && l.contains("GPU-SYNTHETIC-3") && l.contains("code=\"15\"")));
        assert!(output.lines().any(|l|l.starts_with("DCXP_GPU_RECOVERY_ACTION{") && l.contains("GPU-SYNTHETIC-3") && l.contains("pci_bus_id=\"\"") && l.ends_with(" 1")),"unjoined verified identity retains current raw recovery diagnostics");
        assert_eq!(output.lines().filter(|l|l.starts_with("DCXP_GPU_RECOVERY_ACTION_STATUS{") && l.contains("GPU-SYNTHETIC-3")).count(),1);
        std::env::set_var("DCXP_FIXTURE_NESTED_ERROR","999");
        let multiple=super::super::gpu::collect_metrics_from_nvml(&context);
        let (_,diagnostics)=reconcile_nvml_inventory(multiple.gpus,&inventory);
        assert!(diagnostics.iter().any(|error|error.error.query=="pci_info"));
        assert!(diagnostics.iter().any(|error|error.error.query=="recovery_action"));
        assert!(diagnostics.len()>=2);
        let output=PrometheusFormatter::format(&AllMetrics {hostname:"fixture-host".into(),nvml_unmatched_errors:diagnostics,..Default::default()});
        assert_eq!(output.lines().filter(|l|l.starts_with("DCXP_GPU_RECOVERY_ACTION_STATUS{") && l.contains("GPU-SYNTHETIC-3")).count(),1,"one current recovery status even with multiple query errors");
        assert!(!output.lines().any(|l|l.starts_with("GPU_ERROR_STATE{")));
    }

    #[test]
    fn health_definition_inventory_rejects_conflicting_and_duplicate_verified_uuids() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        std::env::set_var("DCXP_FIXTURE_RECOVERY","0");
        let context=super::super::gpu::NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_fixture_path().as_os_str()).unwrap();
        let records=super::super::gpu::collect_metrics_from_nvml(&context).gpus;
        let mut inventory:Vec<_>=(0..8).map(|i|pci_gpu(&format!("0000:{:02x}:00.0",0x50+i))).collect();
        inventory[3].proc_health=Some(ProcGpuHealth {uuid:Some("GPU-SYNTHETIC-3".into()),is_healthy:true,model:None,video_bios:None,firmware:None,failure_reason:None});
        let mut stale=records.clone();stale[3].uuid="GPU-SYNTHETIC-STALE".into();
        let (joined,diagnostics)=reconcile_nvml_inventory(stale,&inventory);
        assert_eq!(joined.len(),7);assert!(!joined.contains_key("0000:53:00.0"));assert!(diagnostics.iter().any(|d|d.uuid=="GPU-SYNTHETIC-STALE" && d.pci_bus_id=="0000:53:00.0"));
        let mut duplicate=records.clone();duplicate[4].uuid=duplicate[3].uuid.clone();
        let (joined,diagnostics)=reconcile_nvml_inventory(duplicate,&inventory);
        assert_eq!(joined.len(),6);assert!(!joined.contains_key("0000:53:00.0"));assert!(!joined.contains_key("0000:54:00.0"));assert!(!diagnostics.is_empty());
        inventory[3].proc_health=None;
        assert_eq!(reconcile_nvml_inventory(records,&inventory).0.len(),8,"unique full BDF remains valid when proc UUID coverage is absent");
    }

    #[test]
    fn health_definition_unique_partial_uuid_carries_unresolved_recovery_without_ambiguous_borrowing() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        std::env::set_var("DCXP_FIXTURE_RECOVERY","1");std::env::set_var("DCXP_FIXTURE_QUERY","pci_info");std::env::set_var("DCXP_FIXTURE_ERROR","999");
        let context=super::super::gpu::NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_fixture_path().as_os_str()).unwrap();
        let native=super::super::gpu::collect_metrics_from_nvml(&context);
        let inventory:Vec<_>=(0..8).map(|i|pci_gpu(&format!("0000:{:02x}:00.0",0x50+i))).collect();
        let (_,diagnostics)=reconcile_nvml_inventory(native.gpus.clone(),&inventory);
        let history=successful_empty_dmesg();let mut tracker=HardwareEvidenceTracker::from_boot_log(&history);
        observe_unmatched_current_health(&diagnostics,&mut tracker);
        std::env::set_var("DCXP_FIXTURE_QUERY","recovery_action");std::env::set_var("DCXP_FIXTURE_ERROR","3");
        let later=super::super::gpu::collect_metrics_from_nvml(&context);
        let (mut joined,_)=reconcile_nvml_inventory(later.gpus.clone(),&inventory);
        let actual=reconcile_nvml_hardware_evidence(joined.remove("0000:53:00.0").unwrap(),&inventory[3],&history,&mut tracker);
        assert_eq!(actual.state.as_u8(),2,"same verified UUID gaining BDF cannot turn unsupported coverage into fault clearance");
        std::env::set_var("DCXP_FIXTURE_QUERY","");std::env::set_var("DCXP_FIXTURE_RECOVERY","0");
        let valid=super::super::gpu::collect_metrics_from_nvml(&context);
        let (mut joined,_)=reconcile_nvml_inventory(valid.gpus,&inventory);
        assert_eq!(reconcile_nvml_hardware_evidence(joined.remove("0000:53:00.0").unwrap(),&inventory[3],&history,&mut tracker).state.as_u8(),1);
        let mut ambiguous=native.gpus;ambiguous[4].uuid=ambiguous[3].uuid.clone();ambiguous[4].pci_bus_id.clear();
        let (_,diagnostics)=reconcile_nvml_inventory(ambiguous,&inventory);
        let mut fresh=HardwareEvidenceTracker::from_boot_log(&history);observe_unmatched_current_health(&diagnostics,&mut fresh);
        let (mut joined,_)=reconcile_nvml_inventory(later.gpus,&inventory);
        assert_eq!(reconcile_nvml_hardware_evidence(joined.remove("0000:53:00.0").unwrap(),&inventory[3],&history,&mut fresh).state.as_u8(),1,"duplicated UUID cannot lend another record's unresolved Reset");
    }

    #[test]
    fn health_definition_review_unchanged_startup_history_survives_unique_ambiguous_unique() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        let pci=pci_gpu("0000:53:00.0");
        let mut raw=successful_empty_dmesg();
        super::super::dmesg::parse_lines("NVRM: Xid (PCI:0000:53:00): 79, omitted",&mut raw);
        let mut tracker=HardwareEvidenceTracker::from_boot_log(&raw);
        for inventory in [vec![pci.clone()],vec![pci.clone(),pci_gpu("0000:53:00.1")],vec![pci.clone()]] {
            let mut snapshot=raw.clone();canonicalize_dmesg_inventory(&mut snapshot,&inventory,&mut tracker);
            let actual=reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&snapshot,&mut tracker);
            assert_eq!(actual.state.as_u8(),1,"unchanged startup history cannot become a new event after inventory ambiguity");
        }
        super::super::dmesg::parse_lines("NVRM: Xid (PCI:0000:53:00): 79, new occurrence",&mut raw);
        canonicalize_dmesg_inventory(&mut raw,&[pci.clone()],&mut tracker);
        assert_eq!(reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&raw,&mut tracker).state.as_u8(),5,"a genuine new occurrence must still latch");
    }
    #[test]
    fn health_definition_review_fallback_rows_retain_nonterminal_xid_snapshot() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        let inventory:Vec<_>=(0..8).map(|i|pci_gpu(&format!("0000:{:02x}:00.0",0x50+i))).collect();
        let collector=MetricsCollectorSync::new(Duration::from_millis(1),false,Arc::new(Mutex::new(HardwareEvidenceTracker::default())));
        for query in ["device_by_index","pci_info"] {
            std::env::set_var("DCXP_FIXTURE_QUERY",query);std::env::set_var("DCXP_FIXTURE_ERROR","15");
            let context=super::super::gpu::NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_fixture_path().as_os_str()).unwrap();
            let native=super::super::gpu::collect_metrics_from_nvml(&context);
            let (mut joined,mut diagnostics)=reconcile_nvml_inventory(native.gpus,&inventory);diagnostics.extend(native.unmatched_errors);
            assert!(!joined.contains_key("0000:53:00.0"));
            let mut history=successful_empty_dmesg();super::super::dmesg::parse_lines("NVRM: Xid (PCI:0000:53:00): 109, omitted\nNVRM: Xid (PCI:0000:53:00): 119, omitted\nNVRM: Xid (PCI:0000:53:00): 154, omitted",&mut history);
            let mut tracker=HardwareEvidenceTracker::from_boot_log(&history);canonicalize_dmesg_inventory(&mut history,&inventory,&mut tracker);
            let gpus:Vec<_>=inventory.iter().enumerate().map(|(i,pci)|{
                let mut gpu=joined.remove(&pci.pci_bus_id).unwrap_or_else(||collector.create_error_gpu(i as u32,pci,"615.71.09"));
                enrich_pci_gpu_snapshot(&mut gpu,pci,&history);gpu
            }).collect();
            assert_eq!(gpus[3].state.as_u8(),2);assert_eq!(gpus[3].xid_errors,vec![109,119,154],"{query} fallback loses observed numeric Xids");
            let output=PrometheusFormatter::format(&AllMetrics {hostname:"fixture-host".into(),gpus,total_pci_gpus:8,nvml_gpus:if query=="device_by_index" {7}else{8},nvml_unmatched_errors:diagnostics,..Default::default()});
            assert_eq!(output.lines().filter(|l|l.starts_with("GPU_ERROR_STATE{")).count(),8);
            for code in [109,119,154] {assert!(output.lines().any(|l|l.starts_with("DCXP_GPU_XID_CODE{") && l.contains("pci_bus_id=\"0000:53:00.0\"") && l.contains(&format!("code=\"{code}\""))));}
            assert!(!output.lines().any(|l|l.starts_with("DCGM_FI_DEV_POWER_USAGE{") && l.contains("gpu=\"3\"")));
            let mut passthrough=collector.create_passthrough_gpu(3,&inventory[3],"615.71.09");
            enrich_pci_gpu_snapshot(&mut passthrough,&inventory[3],&history);assert_eq!(passthrough.state.as_u8(),3);assert_eq!(passthrough.xid_errors,vec![109,119,154]);
        }
    }

    #[test]
    fn health_definition_review_ambiguous_nonterminal_token_cannot_hide_attributed_new_terminal_event() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        let pci=pci_gpu("0000:53:00.0");
        let mut tracker=HardwareEvidenceTracker::from_boot_log(&successful_empty_dmesg());
        let mut snapshot=successful_empty_dmesg();
        super::super::dmesg::parse_lines("NVRM: Xid (PCI:0000:53:00): 154, omitted\nNVRM: Xid (PCI:0000:53:00.0): 79, new",&mut snapshot);
        canonicalize_dmesg_inventory(&mut snapshot,&[pci.clone(),pci_gpu("0000:53:00.1")],&mut tracker);
        let actual=reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&snapshot,&mut tracker);
        assert_eq!(actual.state.as_u8(),5,"ambiguity cannot suppress an independently attributed full-BDF terminal event");
    }

    #[test]
    fn health_definition_review_raw_terminal_provenance_survives_projection_ambiguity() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        let pci=pci_gpu("0000:53:00.0");let mut raw=successful_empty_dmesg();
        super::super::dmesg::parse_lines("NVRM: Xid (PCI:0000:53:00): 79, historical",&mut raw);
        let mut tracker=HardwareEvidenceTracker::from_boot_log(&raw);
        let mut first=raw.clone();canonicalize_dmesg_inventory(&mut first,&[pci.clone()],&mut tracker);
        assert_eq!(reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&first,&mut tracker).state.as_u8(),1);
        super::super::dmesg::parse_lines("NVRM: Xid (PCI:0000:53:00.0): 79, new independently attributed event",&mut raw);
        canonicalize_dmesg_inventory(&mut raw,&[pci.clone(),pci_gpu("0000:53:00.1")],&mut tracker);
        assert_eq!(reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&raw,&mut tracker).state.as_u8(),5,"raw full-BDF event cannot be masked by an earlier projected functionless baseline count");
    }

    #[test]
    fn health_definition_review_unchanged_mixed_raw_tokens_cannot_escalate_on_projection_changes() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        let pci=pci_gpu("0000:53:00.0");let mut raw=successful_empty_dmesg();
        super::super::dmesg::parse_lines("NVRM: Xid (PCI:0000:53:00): 79, historical\nNVRM: Xid (PCI:0000:53:00.0): 79, historical",&mut raw);
        let mut tracker=HardwareEvidenceTracker::from_boot_log(&raw);
        for inventory in [vec![pci.clone()],vec![pci.clone(),pci_gpu("0000:53:00.1")],vec![pci.clone()]] {
            let mut snapshot=raw.clone();canonicalize_dmesg_inventory(&mut snapshot,&inventory,&mut tracker);
            assert_eq!(reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&snapshot,&mut tracker).state.as_u8(),1,"unchanged raw source tokens cannot escalate through merged-count2->1->2 projection");
        }
        let mut fresh=HardwareEvidenceTracker::from_boot_log(&successful_empty_dmesg());
        let mut new_raw=successful_empty_dmesg();super::super::dmesg::parse_lines("NVRM: Xid (PCI:0000:53:00): 79, new unresolved event",&mut new_raw);
        let mut ambiguous=new_raw.clone();canonicalize_dmesg_inventory(&mut ambiguous,&[pci.clone(),pci_gpu("0000:53:00.1")],&mut fresh);
        assert_eq!(reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&ambiguous,&mut fresh).state.as_u8(),1);
        canonicalize_dmesg_inventory(&mut new_raw,&[pci.clone()],&mut fresh);
        assert_eq!(reconcile_nvml_hardware_evidence(health_definition_recovery_gpu(Ok(0)),&pci,&new_raw,&mut fresh).state.as_u8(),5,"unassigned new raw events must remain pending until identity resolves");
    }

}
