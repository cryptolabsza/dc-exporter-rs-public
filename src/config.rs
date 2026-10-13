//! Configuration module for DC Exporter
//!
//! Supports TOML configuration files with sections for:
//! - Agent settings (interval, machine ID)
//! - GPU metrics (which metrics to collect)
//! - System metrics (CPU, memory, network)
//! - IPMI metrics (if available)

use std::path::Path;
use serde::{Deserialize, Serialize};
use anyhow::Result;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub thermal: crate::metrics::thermal::ThermalPolicy,
    #[serde(default)]
    pub agent: AgentConfig,
    
    #[serde(default)]
    pub gpu: GpuMetricsConfig,
    
    #[serde(default)]
    pub system: SystemMetricsConfig,
    
    #[serde(default)]
    pub ipmi: IpmiMetricsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            thermal: crate::metrics::thermal::ThermalPolicy::default(),
            agent: AgentConfig::default(),
            gpu: GpuMetricsConfig::default(),
            system: SystemMetricsConfig::default(),
            ipmi: IpmiMetricsConfig::default(),
        }
    }
}

impl Config {
    /// Load configuration from a TOML file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Machine identifier (defaults to hostname)
    #[serde(default = "default_machine_id")]
    pub machine_id: String,
    
    /// Collection interval in seconds
    #[serde(default = "default_interval")]
    pub interval: u64,
    
    /// NVML timeout in seconds (for detecting hung GPUs)
    #[serde(default = "default_nvml_timeout")]
    pub nvml_timeout: u64,
    
    /// Enable direct register access for VRAM/hotspot temps
    #[serde(default = "default_true")]
    pub enable_register_access: bool,
}

fn default_machine_id() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn default_interval() -> u64 { 10 }
fn default_nvml_timeout() -> u64 { 5 }
fn default_true() -> bool { true }
fn default_false() -> bool { false }

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            machine_id: default_machine_id(),
            interval: default_interval(),
            nvml_timeout: default_nvml_timeout(),
            enable_register_access: true,
        }
    }
}

/// GPU metrics configuration - which metrics to collect
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuMetricsConfig {
    /// Enable GPU metrics collection
    #[serde(default = "default_true")]
    pub enabled: bool,
    
    // DCXP unique metrics (not in dcgm-exporter)
    #[serde(default = "default_true")]
    pub vram_temp: bool,
    
    #[serde(default = "default_true")]
    pub hotspot_temp: bool,
    
    #[serde(default = "default_true")]
    pub throttle_reasons: bool,
    
    #[serde(default = "default_true")]
    pub aer_errors: bool,
    
    #[serde(default = "default_true")]
    pub gpu_state: bool,
    
    // DCGM-compatible metrics (disabled by default to avoid duplicates)
    #[serde(default = "default_false")]
    pub gpu_temp: bool,
    
    #[serde(default = "default_false")]
    pub power_usage: bool,
    
    #[serde(default = "default_true")]
    pub fan_speed: bool,
    
    #[serde(default = "default_false")]
    pub sm_clock: bool,
    
    #[serde(default = "default_false")]
    pub mem_clock: bool,
    
    #[serde(default = "default_false")]
    pub gpu_util: bool,
    
    #[serde(default = "default_false")]
    pub mem_util: bool,
    
    #[serde(default = "default_false")]
    pub fb_used: bool,
    
    #[serde(default = "default_false")]
    pub fb_free: bool,
}

impl Default for GpuMetricsConfig {
    fn default() -> Self {
        GpuMetricsConfig {
            enabled: true,
            // DCXP unique metrics - enabled by default
            vram_temp: true,
            hotspot_temp: true,
            throttle_reasons: true,
            aer_errors: true,
            gpu_state: true,
            fan_speed: true,
            // DCGM-compatible metrics - disabled by default
            gpu_temp: false,
            power_usage: false,
            sm_clock: false,
            mem_clock: false,
            gpu_util: false,
            mem_util: false,
            fb_used: false,
            fb_free: false,
        }
    }
}

/// System metrics configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetricsConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    
    #[serde(default = "default_false")]
    pub cpu_usage: bool,
    
    #[serde(default = "default_false")]
    pub cpu_temp: bool,
    
    #[serde(default = "default_false")]
    pub load_avg: bool,
    
    #[serde(default = "default_false")]
    pub mem_total: bool,
    
    #[serde(default = "default_false")]
    pub mem_used: bool,
    
    #[serde(default = "default_false")]
    pub mem_free: bool,
    
    #[serde(default = "default_false")]
    pub swap_used: bool,
    
    #[serde(default = "default_false")]
    pub net_rx_bytes: bool,
    
    #[serde(default = "default_false")]
    pub net_tx_bytes: bool,
    
    #[serde(default = "default_true")]
    pub apt_upgradable: bool,
}

impl Default for SystemMetricsConfig {
    fn default() -> Self {
        SystemMetricsConfig {
            enabled: false,
            cpu_usage: false,
            cpu_temp: false,
            load_avg: false,
            mem_total: false,
            mem_used: false,
            mem_free: false,
            swap_used: false,
            net_rx_bytes: false,
            net_tx_bytes: false,
            apt_upgradable: true,
        }
    }
}

/// IPMI metrics configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpmiMetricsConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    
    #[serde(default = "default_false")]
    pub cpu_temp: bool,
    
    #[serde(default = "default_false")]
    pub inlet_temp: bool,
    
    #[serde(default = "default_false")]
    pub exhaust_temp: bool,
    
    #[serde(default = "default_false")]
    pub fan_speed: bool,
    
    #[serde(default = "default_false")]
    pub power_consumption: bool,
}

impl Default for IpmiMetricsConfig {
    fn default() -> Self {
        IpmiMetricsConfig {
            enabled: false,
            cpu_temp: false,
            inlet_temp: false,
            exhaust_temp: false,
            fan_speed: false,
            power_consumption: false,
        }
    }
}

/// Generate a default configuration file
pub fn generate_default_config() -> String {
    let config = Config::default();
    toml::to_string_pretty(&config).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.gpu.enabled);
        assert!(config.gpu.vram_temp);
        assert!(config.gpu.hotspot_temp);
        assert!(!config.gpu.gpu_temp); // Should be disabled to avoid duplicates
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.gpu.vram_temp, parsed.gpu.vram_temp);
    }
}
