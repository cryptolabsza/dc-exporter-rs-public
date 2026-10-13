//! Prometheus Metrics Formatter
//!
//! Formats collected metrics in Prometheus exposition format.
//! Uses DCXP_ prefix for unique metrics to distinguish from DCGM.

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
        
        // For VM metrics, update the state to OK (we have data)
        let effective_state = if gpu.source == MetricSource::Vm {
            GpuState::Ok
        } else {
            gpu.state
        };

        // Only a successful current collection may publish operational data.
        // Invalid records retain identity and health evidence, never stale NVML values.
        let operational_metrics_valid = matches!(effective_state, GpuState::Ok | GpuState::Warning)
            && !gpu.nvml_essential_query_failed;

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
}
