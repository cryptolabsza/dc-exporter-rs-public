//! Prometheus Metrics Formatter
//!
//! Formats collected metrics in Prometheus exposition format.
//! Uses DCXP_ prefix for unique metrics to distinguish from DCGM.

// TRACEWEAVER: file-role=vm-health-preserving-formatter; req=VM-FIX-004,VM-FIX-006; trace=TRACE-VM-004,TRACE-VM-006; ver=VER-VM-004,VER-VM-006

use super::collector::count_usable_gpus;
use super::{AllMetrics, GpuMetrics, MetricSource};
use crate::state::GpuState;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_TIMESTAMP: &str = env!("BUILD_TIMESTAMP");
const GIT_HASH: &str = env!("GIT_HASH");

pub struct PrometheusFormatter;

impl PrometheusFormatter {
    /// Format all metrics as Prometheus exposition format
    pub fn format(metrics: &AllMetrics) -> String {
        let mut output = String::with_capacity(16384);

        // Header
        output.push_str(&format!(
            "# DC Exporter v{} - GPU Metrics for Prometheus\n",
            VERSION
        ));
        output.push_str(&format!("# Hostname: {}\n", metrics.hostname));
        output.push_str(&format!("# Timestamp: {}\n", metrics.timestamp));
        output.push('\n');

        // Build info
        output.push_str("# HELP dc_exporter_build_info DC Exporter build information\n");
        output.push_str("# TYPE dc_exporter_build_info gauge\n");
        output.push_str(&format!(
            "dc_exporter_build_info{{version=\"{}\",build_timestamp=\"{}\",git_hash=\"{}\"}} 1\n",
            VERSION,
            BUILD_TIMESTAMP,
            GIT_HASH
        ));
        output.push('\n');

        // GPU availability metrics
        output.push_str("# HELP DCXP_GPU_AVAILABLE Whether GPUs are available to host (0=VM passthrough, 1=available)\n");
        output.push_str("# TYPE DCXP_GPU_AVAILABLE gauge\n");
        output.push_str(&format!(
            "DCXP_GPU_AVAILABLE{{Hostname=\"{}\"}} {}\n",
            metrics.hostname,
            if metrics.gpus_available { 1 } else { 0 }
        ));

        output.push_str("# HELP DCXP_GPU_COUNT GPU count by type: pcie=physical devices, nvml=successfully constructed NVML records, usable=healthy host or VM telemetry plus confirmed passthrough devices\n");
        output.push_str("# TYPE DCXP_GPU_COUNT gauge\n");
        output.push_str(&format!(
            "DCXP_GPU_COUNT{{type=\"pcie\",Hostname=\"{}\"}} {}\n",
            metrics.hostname, metrics.total_pci_gpus
        ));
        output.push_str(&format!(
            "DCXP_GPU_COUNT{{type=\"nvml\",Hostname=\"{}\"}} {}\n",
            metrics.hostname, metrics.nvml_gpus
        ));
        output.push_str(&format!(
            "DCXP_GPU_COUNT{{type=\"usable\",Hostname=\"{}\"}} {}\n",
            metrics.hostname,
            count_usable_gpus(&metrics.gpus)
        ));
        output.push('\n');

        // DCXP metric headers
        Self::write_metric_headers(&mut output);

        // Unjoined diagnostics retain available identity without physical GPU rows.
        let mut recovery_identities=std::collections::HashSet::new();
        for unmatched in &metrics.nvml_unmatched_errors {
            let labels=format!("gpu=\"{}\",UUID=\"{}\",pci_bus_id=\"{}\",device=\"nvidia{}\",modelName=\"{}\",Hostname=\"{}\",DCXP_FI_DRIVER_VERSION=\"{}\"",unmatched.index,unmatched.uuid,unmatched.pci_bus_id,unmatched.index,unmatched.name,metrics.hostname,unmatched.driver_version);
            Self::format_query_error(&mut output,&labels,&unmatched.error);
            if unmatched.recovery.is_some() && recovery_identities.insert((unmatched.index,unmatched.uuid.as_str(),unmatched.pci_bus_id.as_str())) {
                Self::format_recovery_diagnostics(&mut output,unmatched.recovery.as_ref(),&labels);
            }
        }
        // Per-GPU metrics
        for gpu in &metrics.gpus {
            Self::format_gpu_metrics(&mut output, gpu, &metrics.hostname);
        }

        // System metrics
        if let Some(apt) = metrics.apt_upgradable {
            output.push_str("# HELP APT_UPGRADABLE_PACKAGES Number of APT packages that can be upgraded\n");
            output.push_str("# TYPE APT_UPGRADABLE_PACKAGES gauge\n");
            output.push_str(&format!(
                "APT_UPGRADABLE_PACKAGES{{Hostname=\"{}\"}} {}\n",
                metrics.hostname, apt
            ));
        }

        output
    }

    fn write_metric_headers(output: &mut String) {
        for (name,help) in [
            ("DCXP_GPU_RECOVERY_ACTION","Current exact native recovery action code; action is a fixed interpretation, not a physical defect diagnosis."),
            ("DCXP_GPU_RECOVERY_ACTION_STATUS","Current native recovery field coverage and validity."),
            ("DCXP_GPU_NVML_ERROR","Current bounded typed query error; unmatched device_by_index gpu/device labels are enumeration context only, UUID/BDF empty."),
            ("DCXP_GPU_XID_CODE","Occurrences by numeric Xid in current source snapshot, at most64 distinct codes per GPU."),
            ("DCXP_GPU_XID_SOURCE_STATUS","Selected kernel-log read coverage; valid does not establish complete history."),
            ("DCXP_GPU_XID_CODES_TRUNCATED","Whether more than64 distinct current Xid codes were observed for this GPU."),
        ] {output.push_str(&format!("# HELP {} {}\n# TYPE {} gauge\n",name,help,name));}
        // DCXP unique metrics
        output.push_str("# HELP DCXP_FI_DEV_VRAM_TEMP VRAM temperature (in C). 0 if not supported or VM passthrough.\n");
        output.push_str("# TYPE DCXP_FI_DEV_VRAM_TEMP gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_HOT_SPOT_TEMP Hotspot/junction temperature (in C). 0 if not supported or VM passthrough.\n");
        output.push_str("# TYPE DCXP_FI_DEV_HOT_SPOT_TEMP gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_CLOCKS_THROTTLE_REASON Individual throttle reason for GPU clocks.\n");
        output.push_str("# TYPE DCXP_FI_DEV_CLOCKS_THROTTLE_REASON gauge\n");
        output.push_str("# HELP DCXP_GPU_SUPPORTED Whether current VRAM temperature is measured and valid (1=yes, 0=no).\n");
        output.push_str("# TYPE DCXP_GPU_SUPPORTED gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_VRAM_TEMP_VALID Whether current VRAM temperature is valid (1=yes, 0=no).\n");
        output.push_str("# TYPE DCXP_FI_DEV_VRAM_TEMP_VALID gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_VRAM_TEMP_BACKEND_INFO Current VRAM backend and status.\n");
        output.push_str("# TYPE DCXP_FI_DEV_VRAM_TEMP_BACKEND_INFO gauge\n");
        output.push_str("# HELP DCXP_AER_TOTAL_ERRORS Total PCIe AER errors for GPU.\n");
        output.push_str("# TYPE DCXP_AER_TOTAL_ERRORS counter\n");
        output.push_str("# HELP DCXP_GPU_STATE GPU state encoding (0=Unavailable, 1=OK_or_Warning, 2=Error, 3=VM_Passthrough, 5=HW_Failure).\n");
        output.push_str("# TYPE DCXP_GPU_STATE gauge\n");
        output.push_str("# HELP DCXP_GPU_HW_FAILURE GPU hardware failure detected (1=confirmed failure, 0=no failure). Labels include failure_reason.\n");
        output.push_str("# TYPE DCXP_GPU_HW_FAILURE gauge\n");
        output.push_str("# HELP DCXP_GPU_DMESG_RM_INIT_FAILED Count of RmInitAdapter failures in dmesg for this GPU.\n");
        output.push_str("# TYPE DCXP_GPU_DMESG_RM_INIT_FAILED counter\n");
        output.push_str("# HELP DCXP_GPU_FALLEN_OFF_BUS Whether GPU has fallen off the PCIe bus (1=yes, 0=no).\n");
        output.push_str("# TYPE DCXP_GPU_FALLEN_OFF_BUS gauge\n");
        output.push_str("# HELP DCXP_GPU_XID_ERRORS Total XID errors for this GPU from dmesg.\n");
        output.push_str("# TYPE DCXP_GPU_XID_ERRORS counter\n");
        output.push('\n');

        // DCGM-compatible metrics
        output.push_str("# HELP DCGM_FI_DEV_GPU_TEMP GPU temperature (in C).\n");
        output.push_str("# TYPE DCGM_FI_DEV_GPU_TEMP gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_POWER_USAGE GPU power usage (in W).\n");
        output.push_str("# TYPE DCGM_FI_DEV_POWER_USAGE gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_FAN_SPEED GPU fan speed (0-100).\n");
        output.push_str("# TYPE DCGM_FI_DEV_FAN_SPEED gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_SM_CLOCK SM clock frequency (in MHz).\n");
        output.push_str("# TYPE DCGM_FI_DEV_SM_CLOCK gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_MEM_CLOCK Memory clock frequency (in MHz).\n");
        output.push_str("# TYPE DCGM_FI_DEV_MEM_CLOCK gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_GPU_UTIL GPU utilization (0-100).\n");
        output.push_str("# TYPE DCGM_FI_DEV_GPU_UTIL gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_MEM_COPY_UTIL Memory utilization (0-100).\n");
        output.push_str("# TYPE DCGM_FI_DEV_MEM_COPY_UTIL gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_FB_FREE Framebuffer memory free (in MB).\n");
        output.push_str("# TYPE DCGM_FI_DEV_FB_FREE gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_FB_USED Framebuffer memory used (in MB).\n");
        output.push_str("# TYPE DCGM_FI_DEV_FB_USED gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_VRAM_TEMP VRAM temperature (in C). 0 if not supported.\n");
        output.push_str("# TYPE DCGM_FI_DEV_VRAM_TEMP gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_HOT_SPOT_TEMP Hotspot/junction temperature (in C). 0 if not supported.\n");
        output.push_str("# TYPE DCGM_FI_DEV_HOT_SPOT_TEMP gauge\n");
        output.push_str("# HELP DCGM_FI_DEV_CLOCKS_THROTTLE_REASON Individual throttle reason for GPU clocks (1=active, 0=inactive).\n");
        output.push_str("# TYPE DCGM_FI_DEV_CLOCKS_THROTTLE_REASON gauge\n");
        output.push_str("# HELP GPU_AER_TOTAL_ERRORS Total PCIe AER errors for GPU.\n");
        output.push_str("# TYPE GPU_AER_TOTAL_ERRORS counter\n");
        output.push_str("# HELP GPU_ERROR_STATE GPU error state (0=Unknown, 1=No_Error, 2=Error, 3=VM_Passthrough, 5=HW_Failure).\n");
        output.push_str("# TYPE GPU_ERROR_STATE gauge\n");
        output.push('\n');
        
        // Extended metrics
        output.push_str("# HELP DCXP_FI_DEV_POWER_LIMIT GPU power limit (in W).\n");
        output.push_str("# TYPE DCXP_FI_DEV_POWER_LIMIT gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_ENCODER_UTIL Encoder utilization (0-100).\n");
        output.push_str("# TYPE DCXP_FI_DEV_ENCODER_UTIL gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_DECODER_UTIL Decoder utilization (0-100).\n");
        output.push_str("# TYPE DCXP_FI_DEV_DECODER_UTIL gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_PCIE_GEN Current PCIe generation.\n");
        output.push_str("# TYPE DCXP_FI_DEV_PCIE_GEN gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_PCIE_WIDTH Current PCIe link width.\n");
        output.push_str("# TYPE DCXP_FI_DEV_PCIE_WIDTH gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_PCIE_TX_THROUGHPUT PCIe TX throughput (KB/s).\n");
        output.push_str("# TYPE DCXP_FI_DEV_PCIE_TX_THROUGHPUT gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_PCIE_RX_THROUGHPUT PCIe RX throughput (KB/s).\n");
        output.push_str("# TYPE DCXP_FI_DEV_PCIE_RX_THROUGHPUT gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_ECC_SBE Single-bit ECC errors (correctable).\n");
        output.push_str("# TYPE DCXP_FI_DEV_ECC_SBE counter\n");
        output.push_str("# HELP DCXP_FI_DEV_ECC_DBE Double-bit ECC errors (uncorrectable).\n");
        output.push_str("# TYPE DCXP_FI_DEV_ECC_DBE counter\n");
        output.push_str("# HELP DCXP_FI_DEV_NVLINK_ACTIVE Number of active NVLink connections.\n");
        output.push_str("# TYPE DCXP_FI_DEV_NVLINK_ACTIVE gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_NVLINK_TX_THROUGHPUT NVLink TX throughput (KB/s).\n");
        output.push_str("# TYPE DCXP_FI_DEV_NVLINK_TX_THROUGHPUT gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_NVLINK_RX_THROUGHPUT NVLink RX throughput (KB/s).\n");
        output.push_str("# TYPE DCXP_FI_DEV_NVLINK_RX_THROUGHPUT gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_NVLINK_ERRORS Total NVLink errors.\n");
        output.push_str("# TYPE DCXP_FI_DEV_NVLINK_ERRORS counter\n");
        output.push_str("# HELP DCXP_FI_DEV_COMPUTE_PROCS Number of compute processes.\n");
        output.push_str("# TYPE DCXP_FI_DEV_COMPUTE_PROCS gauge\n");
        output.push_str("# HELP DCXP_FI_DEV_GRAPHICS_PROCS Number of graphics processes.\n");
        output.push_str("# TYPE DCXP_FI_DEV_GRAPHICS_PROCS gauge\n");
        output.push('\n');
    }

    fn format_health_diagnostics(output:&mut String,gpu:&GpuMetrics,labels:&str) {
        Self::format_recovery_diagnostics(output,gpu.recovery.as_ref(),labels);
        for error in &gpu.nvml_errors {Self::format_query_error(output,labels,error);}
        let mut codes=std::collections::BTreeMap::<u32,u32>::new();
        for code in &gpu.xid_errors {let count=codes.entry(*code).or_default();*count=count.saturating_add(1);}
        let truncated=codes.len()>64;
        for (code,count) in codes.into_iter().take(64) {output.push_str(&format!("DCXP_GPU_XID_CODE{{{},code=\"{}\"}} {}\n",labels,code,count));}
        let source=match gpu.xid_source {Some(super::dmesg::DmesgSource::FlaggedDmesg)=>"flagged_dmesg",Some(super::dmesg::DmesgSource::PlainDmesg)=>"plain_dmesg",Some(super::dmesg::DmesgSource::KernLog)=>"kern_log",None=>"none"};
        let status=if !gpu.xid_source_read_succeeded {"unavailable"}else if truncated {"truncated"}else{"valid"};
        output.push_str(&format!("DCXP_GPU_XID_SOURCE_STATUS{{{},source=\"{}\",status=\"{}\"}} 1\n",labels,source,status));
        output.push_str(&format!("DCXP_GPU_XID_CODES_TRUNCATED{{{}}} {}\n",labels,u8::from(truncated)));
    }
    fn format_recovery_diagnostics(output:&mut String,recovery:Option<&super::gpu::RecoveryObservation>,labels:&str) {
        if let Some(recovery)=recovery {
            output.push_str(&format!("DCXP_GPU_RECOVERY_ACTION_STATUS{{{},status=\"{}\"}} 1\n",labels,recovery.status.as_str()));
            if let Some(action)=recovery.action {
                let interpretation=match action {0=>"none",1=>"gpu_reset",2=>"node_reboot",3=>"drain_p2p",4=>"drain_and_reset",5=>"recover_imex_domain",6=>"bus_reset",7=>"system_reboot",_=>"unknown"};
                output.push_str(&format!("DCXP_GPU_RECOVERY_ACTION{{{},action=\"{}\"}} {}\n",labels,interpretation,action));
            }
        }
    }
    fn format_query_error(output:&mut String,labels:&str,error:&super::gpu::NvmlQueryError) {
        let code=error.code.map(|code|code.to_string()).unwrap_or_else(||"unknown".into());
        output.push_str(&format!("DCXP_GPU_NVML_ERROR{{{},query=\"{}\",error=\"{}\",code=\"{}\"}} 1\n",labels,error.query,error.error,code));
    }

    // TRACEWEAVER: entrypoint=format_gpu_metrics; req=VM-FIX-004,VM-FIX-006; trace=TRACE-VM-004,TRACE-VM-006; ver=VER-VM-004,VER-VM-006
    fn format_gpu_metrics(output: &mut String, gpu: &GpuMetrics, hostname: &str) {
        // Common labels
        let dcxp_labels = format!(
            "gpu=\"{}\",UUID=\"{}\",pci_bus_id=\"{}\",device=\"nvidia{}\",modelName=\"{}\",Hostname=\"{}\",DCXP_FI_DRIVER_VERSION=\"{}\"",
            gpu.index, gpu.uuid, gpu.pci_bus_id, gpu.index, gpu.name, hostname, gpu.driver_version
        );
        
        let dcgm_labels = format!(
            "gpu=\"{}\",UUID=\"{}\",pci_bus_id=\"{}\",device=\"nvidia{}\",modelName=\"{}\",Hostname=\"{}\",DCGM_FI_DRIVER_VERSION=\"{}\"",
            gpu.index, gpu.uuid, gpu.pci_bus_id, gpu.index, gpu.name, hostname, gpu.driver_version
        );

        // Add source label - important for VM metrics
        let source_label = match gpu.source {
            MetricSource::Host => "",
            MetricSource::Vm => ",source=\"vm\"",  // Metrics collected from inside VM via guest agent
            MetricSource::Register => ",source=\"register\"",
            MetricSource::Pci => ",source=\"pci\"",
        };
        
        // Preserve legacy guest passthrough compatibility without clearing current faults.
        let effective_state = if gpu.source == MetricSource::Vm && gpu.state == GpuState::VmPassthrough {
            GpuState::Ok
        } else {
            gpu.state
        };

        // Only a successful current collection may publish operational data.
        // Invalid records retain identity and health evidence, never stale NVML values.
        let operational_metrics_valid = matches!(effective_state, GpuState::Ok | GpuState::Warning)
            && !gpu.nvml_essential_query_failed;

        // Diagnostic evidence is retained even when operational telemetry is invalid.
        Self::format_health_diagnostics(output,gpu,&dcxp_labels);
        // DCXP unique metrics
        if operational_metrics_valid {
            output.push_str(&format!(
                "DCXP_FI_DEV_VRAM_TEMP{{{}{}}} {}\n",
                dcxp_labels,
                source_label,
                gpu.vram_temp.unwrap_or(0)
            ));

            output.push_str(&format!(
                "DCXP_FI_DEV_HOT_SPOT_TEMP{{{}{}}} {}\n",
                dcxp_labels,
                source_label,
                gpu.hotspot_temp.unwrap_or(0)
            ));

            // GPU supported (for register access)
            let supported = u8::from(super::thermal::measured_vram_supported(gpu));
            output.push_str(&format!(
                "DCXP_GPU_SUPPORTED{{{}}} {}\n",
                dcxp_labels, supported
            ));
        }

        let valid = operational_metrics_valid && super::thermal::measured_vram_supported(gpu);
        let (backend, status) = gpu.vram_thermal.as_ref().map(|m|(m.backend.as_str(),m.status.as_str()))
            .unwrap_or(("legacy", if valid {"valid"} else {"unsupported"}));
        output.push_str(&format!("DCXP_FI_DEV_VRAM_TEMP_VALID{{{dcxp_labels}{source_label}}} {}\n",u8::from(valid)));
        output.push_str(&format!("DCXP_FI_DEV_VRAM_TEMP_BACKEND_INFO{{{dcxp_labels}{source_label},backend=\"{backend}\",status=\"{status}\"}} 1\n"));

        // AER errors
        output.push_str(&format!(
            "DCXP_AER_TOTAL_ERRORS{{{}}} {}\n",
            dcxp_labels, gpu.aer_errors
        ));

        // GPU state (use effective_state which accounts for VM metrics)
        output.push_str(&format!(
            "DCXP_GPU_STATE{{{}}} {}\n",
            dcxp_labels,
            effective_state.as_u8()
        ));

        // Hardware failure detection metrics
        let hw_failure = if gpu.state == GpuState::HardwareFailure { 1 } else { 0 };
        let failure_reason = gpu.hardware_failure_reason.as_deref().unwrap_or("none");
        output.push_str(&format!(
            "DCXP_GPU_HW_FAILURE{{failure_reason=\"{}\",{}}} {}\n",
            failure_reason.replace('"', "'"),
            dcxp_labels,
            hw_failure
        ));
        
        // Dmesg RmInitAdapter failure count
        output.push_str(&format!(
            "DCXP_GPU_DMESG_RM_INIT_FAILED{{{}}} {}\n",
            dcxp_labels,
            gpu.dmesg_rm_init_failed
        ));
        
        // Fallen off bus
        output.push_str(&format!(
            "DCXP_GPU_FALLEN_OFF_BUS{{{}}} {}\n",
            dcxp_labels,
            if gpu.fallen_off_bus { 1 } else { 0 }
        ));
        
        // XID errors count
        output.push_str(&format!(
            "DCXP_GPU_XID_ERRORS{{{}}} {}\n",
            dcxp_labels,
            gpu.xid_errors.len()
        ));

        // Throttle reasons (DCXP and DCGM-compatible)
        if operational_metrics_valid {
            for (reason_name, is_active) in gpu.throttle_reasons.as_pairs() {
                output.push_str(&format!(
                    "DCXP_FI_DEV_CLOCKS_THROTTLE_REASON{{reason=\"{}\",{}}} {}\n",
                    reason_name,
                    dcxp_labels,
                    if is_active { 1 } else { 0 }
                ));
                // DCGM-compatible throttle reason
                output.push_str(&format!(
                    "DCGM_FI_DEV_CLOCKS_THROTTLE_REASON{{reason=\"{}\",{}}} {}\n",
                    reason_name,
                    dcgm_labels,
                    if is_active { 1 } else { 0 }
                ));
            }
        }

        // GPU_AER_TOTAL_ERRORS (simpler metric name for dashboards)
        output.push_str(&format!(
            "GPU_AER_TOTAL_ERRORS{{gpu=\"{}\",UUID=\"{}\",pci_bus_id=\"{}\",Hostname=\"{}\"}} {}\n",
            gpu.index, gpu.uuid, gpu.pci_bus_id, hostname, gpu.aer_errors
        ));

        // GPU_ERROR_STATE (simpler metric name for dashboards)
        output.push_str(&format!(
            "GPU_ERROR_STATE{{gpu=\"{}\",UUID=\"{}\",pci_bus_id=\"{}\",Hostname=\"{}\"}} {}\n",
            gpu.index, gpu.uuid, gpu.pci_bus_id, hostname, effective_state.as_u8()
        ));

        if !operational_metrics_valid {
            output.push('\n');
            return;
        }

        // Extended DCXP metrics
        if let Some(limit) = gpu.power_limit {
            output.push_str(&format!(
                "DCXP_FI_DEV_POWER_LIMIT{{{}}} {:.3}\n",
                dcxp_labels, limit
            ));
        }
        
        if let Some(enc) = gpu.encoder_util {
            output.push_str(&format!(
                "DCXP_FI_DEV_ENCODER_UTIL{{{}}} {}\n",
                dcxp_labels, enc
            ));
        }
        
        if let Some(dec) = gpu.decoder_util {
            output.push_str(&format!(
                "DCXP_FI_DEV_DECODER_UTIL{{{}}} {}\n",
                dcxp_labels, dec
            ));
        }
        
        // PCIe metrics
        if let Some(gen) = gpu.pcie_gen {
            output.push_str(&format!(
                "DCXP_FI_DEV_PCIE_GEN{{{}}} {}\n",
                dcxp_labels, gen
            ));
        }
        if let Some(width) = gpu.pcie_width {
            output.push_str(&format!(
                "DCXP_FI_DEV_PCIE_WIDTH{{{}}} {}\n",
                dcxp_labels, width
            ));
        }
        if let Some(tx) = gpu.pcie_tx_throughput {
            output.push_str(&format!(
                "DCXP_FI_DEV_PCIE_TX_THROUGHPUT{{{}}} {}\n",
                dcxp_labels, tx
            ));
        }
        if let Some(rx) = gpu.pcie_rx_throughput {
            output.push_str(&format!(
                "DCXP_FI_DEV_PCIE_RX_THROUGHPUT{{{}}} {}\n",
                dcxp_labels, rx
            ));
        }
        
        // ECC metrics
        if gpu.ecc_enabled {
            output.push_str(&format!(
                "DCXP_FI_DEV_ECC_SBE{{{}}} {}\n",
                dcxp_labels, gpu.ecc_sbe.unwrap_or(0)
            ));
            output.push_str(&format!(
                "DCXP_FI_DEV_ECC_DBE{{{}}} {}\n",
                dcxp_labels, gpu.ecc_dbe.unwrap_or(0)
            ));
        }
        
        // NVLink metrics
        if gpu.nvlink_active > 0 {
            output.push_str(&format!(
                "DCXP_FI_DEV_NVLINK_ACTIVE{{{}}} {}\n",
                dcxp_labels, gpu.nvlink_active
            ));
            if let Some(tx) = gpu.nvlink_tx_throughput {
                output.push_str(&format!(
                    "DCXP_FI_DEV_NVLINK_TX_THROUGHPUT{{{}}} {}\n",
                    dcxp_labels, tx
                ));
            }
            if let Some(rx) = gpu.nvlink_rx_throughput {
                output.push_str(&format!(
                    "DCXP_FI_DEV_NVLINK_RX_THROUGHPUT{{{}}} {}\n",
                    dcxp_labels, rx
                ));
            }
            if gpu.nvlink_errors > 0 {
                output.push_str(&format!(
                    "DCXP_FI_DEV_NVLINK_ERRORS{{{}}} {}\n",
                    dcxp_labels, gpu.nvlink_errors
                ));
            }
        }
        
        // Process metrics
        output.push_str(&format!(
            "DCXP_FI_DEV_COMPUTE_PROCS{{{}}} {}\n",
            dcxp_labels, gpu.compute_processes
        ));
        output.push_str(&format!(
            "DCXP_FI_DEV_GRAPHICS_PROCS{{{}}} {}\n",
            dcxp_labels, gpu.graphics_processes
        ));

        // DCGM-compatible metrics (for GPUs with data - host or VM)
        if !matches!(effective_state, GpuState::VmPassthrough | GpuState::Unavailable | GpuState::HardwareFailure) {
            if let Some(temp) = gpu.gpu_temp {
                output.push_str(&format!(
                    "DCGM_FI_DEV_GPU_TEMP{{{}}} {}\n",
                    dcgm_labels, temp
                ));
            }

            if let Some(power) = gpu.power_usage {
                output.push_str(&format!(
                    "DCGM_FI_DEV_POWER_USAGE{{{}}} {:.3}\n",
                    dcgm_labels, power
                ));
            }

            // Fan speed - always output (0 for datacenter GPUs without fans)
            output.push_str(&format!(
                "DCGM_FI_DEV_FAN_SPEED{{{}}} {}\n",
                dcgm_labels, gpu.fan_speed.unwrap_or(0)
            ));

            if let Some(sm) = gpu.sm_clock {
                output.push_str(&format!(
                    "DCGM_FI_DEV_SM_CLOCK{{{}}} {}\n",
                    dcgm_labels, sm
                ));
            }

            if let Some(mem) = gpu.mem_clock {
                output.push_str(&format!(
                    "DCGM_FI_DEV_MEM_CLOCK{{{}}} {}\n",
                    dcgm_labels, mem
                ));
            }

            if let Some(util) = gpu.gpu_util {
                output.push_str(&format!(
                    "DCGM_FI_DEV_GPU_UTIL{{{}}} {}\n",
                    dcgm_labels, util
                ));
            }

            if let Some(util) = gpu.mem_util {
                output.push_str(&format!(
                    "DCGM_FI_DEV_MEM_COPY_UTIL{{{}}} {}\n",
                    dcgm_labels, util
                ));
            }

            if let Some(free) = gpu.fb_free {
                output.push_str(&format!(
                    "DCGM_FI_DEV_FB_FREE{{{}}} {}\n",
                    dcgm_labels, free
                ));
            }

            if let Some(used) = gpu.fb_used {
                output.push_str(&format!(
                    "DCGM_FI_DEV_FB_USED{{{}}} {}\n",
                    dcgm_labels, used
                ));
            }

            // VRAM and Hotspot temps - always output to maintain grid (0 if not available)
            output.push_str(&format!(
                "DCGM_FI_DEV_VRAM_TEMP{{{}}} {}\n",
                dcgm_labels, gpu.vram_temp.unwrap_or(0)
            ));
            
            output.push_str(&format!(
                "DCGM_FI_DEV_HOT_SPOT_TEMP{{{}}} {}\n",
                dcgm_labels, gpu.hotspot_temp.unwrap_or(0)
            ));
        } else {
            // For passthrough GPUs, output zeros for DCGM metrics
            output.push_str(&format!(
                "DCGM_FI_DEV_GPU_TEMP{{{}}} 0\n",
                dcgm_labels
            ));
            output.push_str(&format!(
                "DCGM_FI_DEV_POWER_USAGE{{{}}} 0\n",
                dcgm_labels
            ));
            output.push_str(&format!(
                "DCGM_FI_DEV_FAN_SPEED{{{}}} 0\n",
                dcgm_labels
            ));
            output.push_str(&format!(
                "DCGM_FI_DEV_SM_CLOCK{{{}}} 0\n",
                dcgm_labels
            ));
            output.push_str(&format!(
                "DCGM_FI_DEV_MEM_CLOCK{{{}}} 0\n",
                dcgm_labels
            ));
            output.push_str(&format!(
                "DCGM_FI_DEV_GPU_UTIL{{{}}} 0\n",
                dcgm_labels
            ));
            output.push_str(&format!(
                "DCGM_FI_DEV_MEM_COPY_UTIL{{{}}} 0\n",
                dcgm_labels
            ));
            output.push_str(&format!(
                "DCGM_FI_DEV_FB_FREE{{{}}} 0\n",
                dcgm_labels
            ));
            output.push_str(&format!(
                "DCGM_FI_DEV_FB_USED{{{}}} 0\n",
                dcgm_labels
            ));
            output.push_str(&format!(
                "DCGM_FI_DEV_VRAM_TEMP{{{}}} 0\n",
                dcgm_labels
            ));
            output.push_str(&format!(
                "DCGM_FI_DEV_HOT_SPOT_TEMP{{{}}} 0\n",
                dcgm_labels
            ));
        }

        output.push('\n');
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ThrottleReasons;

    fn has_sample(output: &str, metric: &str, uuid: &str) -> bool {
        output
            .lines()
            .any(|line| line.starts_with(&format!("{}{{", metric)) && line.contains(uuid))
    }

    #[test]
    fn test_empty_metrics() {
        let metrics = AllMetrics {
            hostname: "test-host".to_string(),
            ..Default::default()
        };
        
        let output = PrometheusFormatter::format(&metrics);
        assert!(output.contains("dc_exporter_build_info"));
        assert!(output.contains("DCXP_GPU_AVAILABLE"));
    }

    #[test]
    fn test_gpu_metrics() {
        let gpu = GpuMetrics {
            index: 0,
            name: "Test GPU".to_string(),
            uuid: "GPU-TEST-UUID".to_string(),
            pci_bus_id: "0000:01:00.0".to_string(),
            driver_version: "535.86.05".to_string(),
            state: GpuState::Ok,
            gpu_temp: Some(65),
            vram_temp: Some(72),
            ..Default::default()
        };

        let metrics = AllMetrics {
            hostname: "test-host".to_string(),
            gpus: vec![gpu],
            gpus_available: true,
            nvml_gpus: 1,
            total_pci_gpus: 1,
            ..Default::default()
        };

        let output = PrometheusFormatter::format(&metrics);
        assert!(output.contains("DCXP_FI_DEV_VRAM_TEMP"));
        assert!(output.contains("72"));
        assert!(output.contains("DCGM_FI_DEV_GPU_TEMP"));
        assert!(output.contains("65"));
    }

    #[test]
    fn gpu_counts_distinguish_constructed_nvml_records_from_usable_telemetry() {
        let healthy = GpuMetrics {
            index: 0,
            state: GpuState::Ok,
            source: MetricSource::Host,
            ..Default::default()
        };
        let invalid = GpuMetrics {
            index: 1,
            state: GpuState::Error,
            source: MetricSource::Host,
            nvml_essential_query_failed: true,
            ..Default::default()
        };
        let valid_vm = GpuMetrics {
            index: 2,
            state: GpuState::Ok,
            source: MetricSource::Vm,
            ..Default::default()
        };
        let confirmed_passthrough = GpuMetrics {
            index: 3,
            state: GpuState::VmPassthrough,
            source: MetricSource::Pci,
            ..Default::default()
        };
        let metrics = AllMetrics {
            hostname: "runpodccc95".to_string(),
            total_pci_gpus: 4,
            nvml_gpus: 2,
            gpus_available: true,
            gpus: vec![healthy, invalid, valid_vm, confirmed_passthrough],
            ..Default::default()
        };

        let output = PrometheusFormatter::format(&metrics);
        assert!(output.contains("# HELP DCXP_GPU_COUNT GPU count by type: pcie=physical devices, nvml=successfully constructed NVML records, usable=healthy host or VM telemetry plus confirmed passthrough devices"));
        assert!(output.contains("DCXP_GPU_COUNT{type=\"pcie\",Hostname=\"runpodccc95\"} 4"));
        assert!(output.contains("DCXP_GPU_COUNT{type=\"nvml\",Hostname=\"runpodccc95\"} 2"));
        assert!(output.contains("DCXP_GPU_COUNT{type=\"usable\",Hostname=\"runpodccc95\"} 3"));
        assert!(output.contains("DCXP_GPU_AVAILABLE{Hostname=\"runpodccc95\"} 1"));
    }

    #[test]
    fn gpu_state_help_and_samples_match_dashboard_encoding() {
        let cases = [
            (GpuState::Ok, 1),
            (GpuState::Warning, 1),
            (GpuState::Error, 2),
            (GpuState::VmPassthrough, 3),
            (GpuState::Unavailable, 0),
            (GpuState::HardwareFailure, 5),
        ];
        let gpus = cases
            .iter()
            .enumerate()
            .map(|(index, (state, _))| GpuMetrics {
                index: index as u32,
                uuid: format!("GPU-STATE-{state:?}"),
                state: *state,
                source: if *state == GpuState::VmPassthrough {
                    MetricSource::Pci
                } else {
                    MetricSource::Host
                },
                ..Default::default()
            })
            .collect();
        let output = PrometheusFormatter::format(&AllMetrics {
            hostname: "state-contract".to_string(),
            gpus,
            ..Default::default()
        });

        assert!(output.contains("# HELP DCXP_GPU_STATE GPU state encoding (0=Unavailable, 1=OK_or_Warning, 2=Error, 3=VM_Passthrough, 5=HW_Failure)."));
        for (state, expected) in cases {
            let uuid = format!("GPU-STATE-{state:?}");
            let sample = output
                .lines()
                .find(|line| line.starts_with("DCXP_GPU_STATE{") && line.contains(&uuid))
                .unwrap();
            assert!(
                sample.ends_with(&format!("}} {expected}")),
                "{state:?} emitted the wrong state sample: {sample}"
            );
        }
    }

    #[test]
    fn failed_gpus_export_health_only_even_if_stale_telemetry_is_populated() {
        for (state, nvml_essential_query_failed) in [
            (GpuState::Error, false),
            (GpuState::HardwareFailure, false),
            (GpuState::Warning, true),
        ] {
            let uuid = format!("GPU-{state:?}");
            let gpu = GpuMetrics {
                index: 1,
                name: "NVIDIA GeForce RTX 5090".to_string(),
                uuid: uuid.clone(),
                pci_bus_id: "0000:23:00.0".to_string(),
                driver_version: "590.48.01".to_string(),
                state,
                nvml_essential_query_failed,
                gpu_temp: Some(58),
                vram_temp: Some(127),
                hotspot_temp: Some(91),
                power_usage: Some(530.377),
                power_limit: Some(575.0),
                sm_clock: Some(2_400),
                gpu_util: Some(98),
                fb_used: Some(31_000),
                pcie_gen: Some(5),
                encoder_util: Some(77),
                compute_processes: 2,
                throttle_reasons: ThrottleReasons {
                    sw_power_cap: true,
                    hw_slowdown: true,
                    hw_thermal_slowdown: true,
                    ..Default::default()
                },
                hardware_failure_reason: Some("GPU fallen off bus".to_string()),
                dmesg_hardware_failure: true,
                fallen_off_bus: true,
                xid_errors: vec![79],
                ..Default::default()
            };
            let metrics = AllMetrics {
                hostname: "runpodccc95".to_string(),
                gpus: vec![gpu],
                ..Default::default()
            };

            let output = PrometheusFormatter::format(&metrics);

            for metric in [
                "DCXP_FI_DEV_VRAM_TEMP",
                "DCXP_FI_DEV_HOT_SPOT_TEMP",
                "DCXP_FI_DEV_CLOCKS_THROTTLE_REASON",
                "DCGM_FI_DEV_CLOCKS_THROTTLE_REASON",
                "DCXP_FI_DEV_POWER_LIMIT",
                "DCXP_FI_DEV_ENCODER_UTIL",
                "DCXP_FI_DEV_PCIE_GEN",
                "DCXP_FI_DEV_COMPUTE_PROCS",
                "DCGM_FI_DEV_GPU_TEMP",
                "DCGM_FI_DEV_POWER_USAGE",
                "DCGM_FI_DEV_SM_CLOCK",
                "DCGM_FI_DEV_GPU_UTIL",
                "DCGM_FI_DEV_FB_USED",
                "DCGM_FI_DEV_VRAM_TEMP",
            ] {
                assert!(
                    !has_sample(&output, metric, &uuid),
                    "{state:?} unexpectedly exported {metric}:\n{output}"
                );
            }

            for metric in [
                "DCXP_GPU_STATE",
                "DCXP_GPU_HW_FAILURE",
                "DCXP_GPU_FALLEN_OFF_BUS",
                "DCXP_GPU_XID_ERRORS",
                "GPU_ERROR_STATE",
            ] {
                assert!(
                    has_sample(&output, metric, &uuid),
                    "{state:?} omitted health metric {metric}:\n{output}"
                );
            }
        }
    }

    // TRACEWEAVER: verification=vm_formatter_current_faults_preserve_health_and_suppress_stale_values; req=VM-FIX-004,VM-FIX-006; trace=TRACE-VM-004,TRACE-VM-006; ver=VER-VM-004,VER-VM-006
    #[test]
    fn vm_formatter_current_faults_preserve_health_and_suppress_stale_values() {
        for (state, expected) in [
            (GpuState::HardwareFailure, 5),
            (GpuState::Error, 2),
            (GpuState::Unavailable, 0),
        ] {
            let uuid = format!("GPU-VM-{state:?}");
            let output = PrometheusFormatter::format(&AllMetrics {
                hostname: "vm-health-fixture".into(),
                gpus: vec![GpuMetrics {
                    index: 3,
                    name: "Guest GPU".into(),
                    uuid: uuid.clone(),
                    pci_bus_id: "0000:23:00.0".into(),
                    driver_version: "fixture-driver".into(),
                    state,
                    source: MetricSource::Vm,
                    // Deliberately stale values: the formatter must enforce health itself.
                    gpu_temp: Some(58),
                    vram_temp: Some(72),
                    hotspot_temp: Some(80),
                    power_usage: Some(530.377),
                    power_limit: Some(575.0),
                    sm_clock: Some(2_400),
                    mem_clock: Some(10_000),
                    gpu_util: Some(98),
                    mem_util: Some(83),
                    encoder_util: Some(77),
                    decoder_util: Some(66),
                    fb_used: Some(31_000),
                    fb_free: Some(1_000),
                    fan_speed: Some(75),
                    pcie_gen: Some(5),
                    pcie_width: Some(16),
                    pcie_tx_throughput: Some(100),
                    pcie_rx_throughput: Some(200),
                    ecc_enabled: true,
                    ecc_sbe: Some(3),
                    ecc_dbe: Some(4),
                    nvlink_active: 1,
                    nvlink_tx_throughput: Some(300),
                    nvlink_rx_throughput: Some(400),
                    nvlink_errors: 2,
                    compute_processes: 2,
                    graphics_processes: 1,
                    throttle_reasons: ThrottleReasons {
                        hw_thermal_slowdown: true,
                        ..Default::default()
                    },
                    aer_errors: 7,
                    hardware_failure_reason: Some("current host fault".into()),
                    dmesg_rm_init_failed: 2,
                    fallen_off_bus: true,
                    xid_errors: vec![79],
                    xid_source: Some(super::super::dmesg::DmesgSource::FlaggedDmesg),
                    xid_source_read_succeeded: true,
                    recovery: Some(super::super::gpu::RecoveryObservation {
                        action: Some(1),
                        status: super::super::gpu::RecoveryStatus::Valid,
                        error: None,
                    }),
                    nvml_errors: vec![super::super::gpu::NvmlQueryError {
                        query: "power_usage", error: "gpu_lost", code: Some(15),
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            });

            for metric in ["DCXP_GPU_STATE", "GPU_ERROR_STATE"] {
                let sample = output.lines().find(|line| {
                    line.starts_with(&format!("{metric}{{")) && line.contains(&uuid)
                }).unwrap();
                assert!(sample.ends_with(&format!("}} {expected}")), "{state:?}: {sample}");
                for identity in ["pci_bus_id=\"0000:23:00.0\"", "Hostname=\"vm-health-fixture\""] {
                    assert!(sample.contains(identity), "lost VM identity: {sample}");
                }
            }
            for (metric, suffix) in [
                ("DCXP_FI_DEV_VRAM_TEMP_VALID", " 0"),
                ("DCXP_FI_DEV_VRAM_TEMP_BACKEND_INFO", " 1"),
            ] {
                assert!(output.lines().any(|line| line.starts_with(&format!("{metric}{{"))
                    && line.contains(&uuid) && line.contains("source=\"vm\"")
                    && line.ends_with(suffix)), "lost VM source/validity for {state:?}: {metric}");
            }
            for (metric, label, value) in [
                ("DCXP_GPU_HW_FAILURE", "failure_reason=\"current host fault\"", if expected == 5 { 1 } else { 0 }),
                ("DCXP_GPU_DMESG_RM_INIT_FAILED", "pci_bus_id=\"0000:23:00.0\"", 2),
                ("DCXP_GPU_FALLEN_OFF_BUS", "pci_bus_id=\"0000:23:00.0\"", 1),
                ("DCXP_AER_TOTAL_ERRORS", "pci_bus_id=\"0000:23:00.0\"", 7),
                ("DCXP_GPU_XID_ERRORS", "pci_bus_id=\"0000:23:00.0\"", 1),
                ("DCXP_GPU_XID_CODE", "code=\"79\"", 1),
                ("DCXP_GPU_XID_SOURCE_STATUS", "source=\"flagged_dmesg\",status=\"valid\"", 1),
                ("DCXP_GPU_RECOVERY_ACTION", "action=\"gpu_reset\"", 1),
                ("DCXP_GPU_NVML_ERROR", "query=\"power_usage\",error=\"gpu_lost\",code=\"15\"", 1),
            ] {
                assert!(output.lines().any(|line| line.starts_with(&format!("{metric}{{"))
                    && line.contains(&uuid) && line.contains(label)
                    && line.ends_with(&format!("}} {value}"))), "lost {metric} diagnostic for {state:?}");
            }
            for line in output.lines().filter(|line| line.contains(&uuid)) {
                let operational = line.starts_with("DCGM_FI_DEV_")
                    || line.starts_with("DCXP_GPU_SUPPORTED{")
                    || (line.starts_with("DCXP_FI_DEV_")
                        && !line.starts_with("DCXP_FI_DEV_VRAM_TEMP_VALID{")
                        && !line.starts_with("DCXP_FI_DEV_VRAM_TEMP_BACKEND_INFO{"));
                assert!(!operational, "{state:?} exported stale operational sample: {line}");
            }
        }
    }

    // TRACEWEAVER: verification=vm_formatter_nominal_and_legacy_passthrough_remain_compatible; req=VM-FIX-006; trace=TRACE-VM-006; ver=VER-VM-006
    #[test]
    fn vm_formatter_nominal_and_legacy_passthrough_remain_compatible() {
        for state in [GpuState::Ok, GpuState::Warning, GpuState::VmPassthrough] {
            let uuid = format!("GPU-VM-NOMINAL-{state:?}");
            let output = PrometheusFormatter::format(&AllMetrics {
                gpus: vec![GpuMetrics {
                    uuid: uuid.clone(),
                    state,
                    source: MetricSource::Vm,
                    gpu_temp: Some(58),
                    vram_temp: Some(72),
                    hotspot_temp: Some(80),
                    ..Default::default()
                }],
                ..Default::default()
            });
            for (metric, value) in [
                ("DCXP_GPU_STATE", 1), ("GPU_ERROR_STATE", 1),
                ("DCXP_FI_DEV_VRAM_TEMP", 72), ("DCGM_FI_DEV_GPU_TEMP", 58),
                ("DCGM_FI_DEV_HOT_SPOT_TEMP", 80), ("DCXP_FI_DEV_VRAM_TEMP_VALID", 1),
            ] {
                assert!(output.lines().any(|line| line.starts_with(&format!("{metric}{{"))
                    && line.contains(&uuid) && line.ends_with(&format!("}} {value}"))),
                    "{state:?} lost compatible {metric} value {value}");
            }
            assert!(output.lines().any(|line| line.starts_with("DCXP_FI_DEV_VRAM_TEMP{")
                && line.contains(&uuid) && line.contains("source=\"vm\"")));
        }
    }

    // VER-HEALTH-005: exact new snapshot contract against production formatter.
    #[test]
    fn health_definition_numeric_xid_gauges_survive_fault_telemetry_suppression() {
        let metrics=AllMetrics {hostname:"fixture-host".into(),gpus:vec![GpuMetrics {
            uuid:"GPU-SYNTHETIC".into(),pci_bus_id:"0000:61:00.0".into(),
            state:GpuState::Error,xid_errors:vec![109,119,154,154],..Default::default()
        }],..Default::default()};
        let first=PrometheusFormatter::format(&metrics);
        let second=PrometheusFormatter::format(&metrics);
        assert_eq!(first,second);
        assert!(first.contains("# TYPE DCXP_GPU_XID_CODE gauge\n"));
        for (code,count) in [(109,1),(119,1),(154,2)] {
            let samples:Vec<_>=first.lines().filter(|line|line.starts_with("DCXP_GPU_XID_CODE{") && line.contains(&format!("code=\"{code}\""))).collect();
            assert_eq!(samples.len(),1,"{code}");
            assert!(samples[0].ends_with(&format!(" {count}")));
            assert!(samples[0].contains("UUID=\"GPU-SYNTHETIC\""));
        }
        assert!(!has_sample(&first,"DCGM_FI_DEV_POWER_USAGE","GPU-SYNTHETIC"));
    }

    #[test]
    fn health_definition_xid_cardinality_is_64_with_explicit_truncation() {
        let output=PrometheusFormatter::format(&AllMetrics {hostname:"fixture-host".into(),gpus:vec![GpuMetrics {
            uuid:"GPU-SYNTHETIC".into(),xid_errors:(1..=65).collect(),..Default::default()
        }],..Default::default()});
        assert_eq!(output.lines().filter(|line|line.starts_with("DCXP_GPU_XID_CODE{")).count(),64);
        assert!(output.lines().any(|line|line.starts_with("DCXP_GPU_XID_CODES_TRUNCATED{") && line.ends_with(" 1")));
    }

// VER-HEALTH-005: exact additive action/status/error samples from actual adapter.
    #[test]
    fn health_definition_future_formatter_retains_diagnostics_without_operational_values() {
        let _guard=crate::metrics::gpu::tests::health_definition_fixture_lock();
        for key in ["DCXP_FIXTURE_TAG","DCXP_FIXTURE_BAD_FIELD","DCXP_FIXTURE_NESTED_ERROR","DCXP_FIXTURE_UNTOUCHED"] {std::env::remove_var(key);}
        std::env::set_var("DCXP_FIXTURE_RECOVERY","1");std::env::set_var("DCXP_FIXTURE_QUERY","power_usage");std::env::set_var("DCXP_FIXTURE_ERROR","15");
        let context=crate::metrics::gpu::NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_fixture_path().as_os_str()).unwrap();
        let gpu=crate::metrics::gpu::collect_metrics_from_nvml(&context).gpus.remove(3);
        let output=PrometheusFormatter::format(&AllMetrics {hostname:"fixture-host".into(),gpus:vec![gpu],..Default::default()});
        for metric in ["DCXP_GPU_RECOVERY_ACTION","DCXP_GPU_RECOVERY_ACTION_STATUS","DCXP_GPU_NVML_ERROR"] {assert!(output.contains(&format!("# TYPE {metric} gauge\n")));}
        assert!(output.lines().any(|l|l.starts_with("DCXP_GPU_RECOVERY_ACTION{") && l.contains("action=\"gpu_reset\"") && l.ends_with(" 1")));
        assert!(output.lines().any(|l|l.starts_with("DCXP_GPU_RECOVERY_ACTION_STATUS{") && l.contains("status=\"valid\"") && l.ends_with(" 1")));
        assert!(output.lines().any(|l|l.starts_with("DCXP_GPU_NVML_ERROR{") && l.contains("query=\"power_usage\"") && l.contains("error=\"gpu_lost\"") && l.contains("code=\"15\"") && l.ends_with(" 1")));
        assert!(!has_sample(&output,"DCGM_FI_DEV_POWER_USAGE","GPU-SYNTHETIC-3"));
    }

}
