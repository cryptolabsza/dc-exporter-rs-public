//! TUI Application State and Main Loop

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::metrics::collector::MetricsCollector;
use crate::metrics::{AllMetrics, GpuMetrics, RegisterAccessStatus};

use super::ui;

/// History length for sparkline graphs
const HISTORY_LEN: usize = 60;

/// Display settings - which metrics to show (like htop setup)
#[derive(Clone)]
pub struct DisplaySettings {
    // Temperatures
    pub show_gpu_temp: bool,
    pub show_vram_temp: bool,
    pub show_hotspot_temp: bool,
    
    // Power
    pub show_power: bool,
    pub show_power_limit: bool,
    
    // Utilization
    pub show_gpu_util: bool,
    pub show_mem_util: bool,
    pub show_encoder: bool,
    pub show_decoder: bool,
    
    // Memory
    pub show_vram_usage: bool,
    
    // Clocks
    pub show_clocks: bool,
    pub show_fan: bool,
    
    // PCIe
    pub show_pcie_info: bool,
    pub show_pcie_throughput: bool,
    
    // NVLink
    pub show_nvlink: bool,
    
    // ECC
    pub show_ecc: bool,
    
    // Processes
    pub show_process_count: bool,
    
    // Throttle
    pub show_throttle: bool,
    
    // Graphs
    pub show_graphs: bool,
    pub show_graph_stats: bool,
    
    // Errors
    pub show_aer_errors: bool,
    pub show_state: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            // All enabled by default
            show_gpu_temp: true,
            show_vram_temp: true,
            show_hotspot_temp: true,
            show_power: true,
            show_power_limit: true,
            show_gpu_util: true,
            show_mem_util: true,
            show_encoder: true,
            show_decoder: true,
            show_vram_usage: true,
            show_clocks: true,
            show_fan: true,
            show_pcie_info: true,
            show_pcie_throughput: true,
            show_nvlink: true,
            show_ecc: true,
            show_process_count: true,
            show_throttle: true,
            show_graphs: true,
            show_graph_stats: true,
            show_aer_errors: true,
            show_state: true,
        }
    }
}

impl DisplaySettings {
    /// Get all settings as a list of (name, enabled, description)
    pub fn as_list(&self) -> Vec<(&'static str, bool, &'static str)> {
        vec![
            ("GPU Temperature", self.show_gpu_temp, "Core GPU temperature"),
            ("VRAM Temperature", self.show_vram_temp, "Video memory temperature"),
            ("Hotspot Temperature", self.show_hotspot_temp, "Junction/hotspot temperature"),
            ("Power Usage", self.show_power, "Current power draw"),
            ("Power Limit", self.show_power_limit, "Power limit/cap"),
            ("GPU Utilization", self.show_gpu_util, "GPU compute utilization"),
            ("Memory Utilization", self.show_mem_util, "Memory controller utilization"),
            ("Encoder Util", self.show_encoder, "NVENC encoder usage"),
            ("Decoder Util", self.show_decoder, "NVDEC decoder usage"),
            ("VRAM Usage", self.show_vram_usage, "Video memory used/free"),
            ("Clocks", self.show_clocks, "SM and memory clock speeds"),
            ("Fan Speed", self.show_fan, "Fan speed percentage"),
            ("PCIe Info", self.show_pcie_info, "PCIe generation and width"),
            ("PCIe Throughput", self.show_pcie_throughput, "PCIe TX/RX bandwidth"),
            ("NVLink", self.show_nvlink, "NVLink status and throughput"),
            ("ECC Errors", self.show_ecc, "ECC error counts"),
            ("Process Count", self.show_process_count, "Running compute/graphics processes"),
            ("Throttle Reasons", self.show_throttle, "Clock throttling indicators"),
            ("Graphs", self.show_graphs, "Sparkline history graphs"),
            ("Graph Stats", self.show_graph_stats, "Min/max/avg on graphs"),
            ("AER Errors", self.show_aer_errors, "PCIe AER error count"),
            ("GPU State", self.show_state, "GPU health state indicator"),
        ]
    }
    
    /// Toggle a setting by index
    pub fn toggle(&mut self, index: usize) {
        match index {
            0 => self.show_gpu_temp = !self.show_gpu_temp,
            1 => self.show_vram_temp = !self.show_vram_temp,
            2 => self.show_hotspot_temp = !self.show_hotspot_temp,
            3 => self.show_power = !self.show_power,
            4 => self.show_power_limit = !self.show_power_limit,
            5 => self.show_gpu_util = !self.show_gpu_util,
            6 => self.show_mem_util = !self.show_mem_util,
            7 => self.show_encoder = !self.show_encoder,
            8 => self.show_decoder = !self.show_decoder,
            9 => self.show_vram_usage = !self.show_vram_usage,
            10 => self.show_clocks = !self.show_clocks,
            11 => self.show_fan = !self.show_fan,
            12 => self.show_pcie_info = !self.show_pcie_info,
            13 => self.show_pcie_throughput = !self.show_pcie_throughput,
            14 => self.show_nvlink = !self.show_nvlink,
            15 => self.show_ecc = !self.show_ecc,
            16 => self.show_process_count = !self.show_process_count,
            17 => self.show_throttle = !self.show_throttle,
            18 => self.show_graphs = !self.show_graphs,
            19 => self.show_graph_stats = !self.show_graph_stats,
            20 => self.show_aer_errors = !self.show_aer_errors,
            21 => self.show_state = !self.show_state,
            _ => {}
        }
    }
    
    /// Enable all settings
    pub fn enable_all(&mut self) {
        *self = Self {
            show_gpu_temp: true,
            show_vram_temp: true,
            show_hotspot_temp: true,
            show_power: true,
            show_power_limit: true,
            show_gpu_util: true,
            show_mem_util: true,
            show_encoder: true,
            show_decoder: true,
            show_vram_usage: true,
            show_clocks: true,
            show_fan: true,
            show_pcie_info: true,
            show_pcie_throughput: true,
            show_nvlink: true,
            show_ecc: true,
            show_process_count: true,
            show_throttle: true,
            show_graphs: true,
            show_graph_stats: true,
            show_aer_errors: true,
            show_state: true,
        };
    }
    
    /// Disable all settings
    pub fn disable_all(&mut self) {
        *self = Self {
            show_gpu_temp: false,
            show_vram_temp: false,
            show_hotspot_temp: false,
            show_power: false,
            show_power_limit: false,
            show_gpu_util: false,
            show_mem_util: false,
            show_encoder: false,
            show_decoder: false,
            show_vram_usage: false,
            show_clocks: false,
            show_fan: false,
            show_pcie_info: false,
            show_pcie_throughput: false,
            show_nvlink: false,
            show_ecc: false,
            show_process_count: false,
            show_throttle: false,
            show_graphs: false,
            show_graph_stats: false,
            show_aer_errors: false,
            show_state: false,
        };
    }
    
    /// Count enabled settings
    pub fn enabled_count(&self) -> usize {
        self.as_list().iter().filter(|(_, enabled, _)| *enabled).count()
    }
}

/// GPU history data for graphs
#[derive(Clone, Default)]
pub struct GpuHistory {
    pub gpu_temp: VecDeque<u64>,
    pub vram_temp: VecDeque<u64>,
    pub hotspot_temp: VecDeque<u64>,
    pub gpu_util: VecDeque<u64>,
    pub mem_util: VecDeque<u64>,
    pub power: VecDeque<u64>,
    pub encoder_util: VecDeque<u64>,
    pub decoder_util: VecDeque<u64>,
    pub pcie_tx: VecDeque<u64>,
    pub pcie_rx: VecDeque<u64>,
    pub nvlink_tx: VecDeque<u64>,
    pub nvlink_rx: VecDeque<u64>,
}

impl GpuHistory {
    pub fn new() -> Self {
        Self {
            gpu_temp: VecDeque::with_capacity(HISTORY_LEN),
            vram_temp: VecDeque::with_capacity(HISTORY_LEN),
            hotspot_temp: VecDeque::with_capacity(HISTORY_LEN),
            gpu_util: VecDeque::with_capacity(HISTORY_LEN),
            mem_util: VecDeque::with_capacity(HISTORY_LEN),
            power: VecDeque::with_capacity(HISTORY_LEN),
            encoder_util: VecDeque::with_capacity(HISTORY_LEN),
            decoder_util: VecDeque::with_capacity(HISTORY_LEN),
            pcie_tx: VecDeque::with_capacity(HISTORY_LEN),
            pcie_rx: VecDeque::with_capacity(HISTORY_LEN),
            nvlink_tx: VecDeque::with_capacity(HISTORY_LEN),
            nvlink_rx: VecDeque::with_capacity(HISTORY_LEN),
        }
    }

    pub fn push(&mut self, gpu: &GpuMetrics) {
        // Helper to maintain fixed size
        fn push_value(history: &mut VecDeque<u64>, value: u64) {
            if history.len() >= HISTORY_LEN {
                history.pop_front();
            }
            history.push_back(value);
        }

        push_value(&mut self.gpu_temp, gpu.gpu_temp.unwrap_or(0) as u64);
        push_value(&mut self.vram_temp, gpu.vram_temp.unwrap_or(0) as u64);
        push_value(&mut self.hotspot_temp, gpu.hotspot_temp.unwrap_or(0) as u64);
        push_value(&mut self.gpu_util, gpu.gpu_util.unwrap_or(0) as u64);
        push_value(&mut self.mem_util, gpu.mem_util.unwrap_or(0) as u64);
        push_value(&mut self.power, gpu.power_usage.unwrap_or(0.0) as u64);
        push_value(&mut self.encoder_util, gpu.encoder_util.unwrap_or(0) as u64);
        push_value(&mut self.decoder_util, gpu.decoder_util.unwrap_or(0) as u64);
        push_value(&mut self.pcie_tx, gpu.pcie_tx_throughput.unwrap_or(0) / 1024); // KB/s -> MB/s
        push_value(&mut self.pcie_rx, gpu.pcie_rx_throughput.unwrap_or(0) / 1024);
        push_value(&mut self.nvlink_tx, gpu.nvlink_tx_throughput.unwrap_or(0) / 1024);
        push_value(&mut self.nvlink_rx, gpu.nvlink_rx_throughput.unwrap_or(0) / 1024);
    }
    
    /// Get min/max/avg for a history
    pub fn stats(data: &VecDeque<u64>) -> (u64, u64, u64) {
        if data.is_empty() {
            return (0, 0, 0);
        }
        let min = *data.iter().min().unwrap_or(&0);
        let max = *data.iter().max().unwrap_or(&0);
        let avg = data.iter().sum::<u64>() / data.len() as u64;
        (min, max, avg)
    }
}

/// View mode for the TUI
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    List,       // GPU list with compact cards
    Detail,     // Detailed view of selected GPU
    Processes,  // Process list view
    Settings,   // Settings/setup view (like htop F2)
}

/// Application state
pub struct App {
    /// Current metrics
    pub metrics: AllMetrics,
    /// Per-GPU history for graphs
    pub gpu_history: Vec<GpuHistory>,
    /// Currently selected GPU (for detailed view)
    pub selected_gpu: usize,
    /// Show help overlay
    pub show_help: bool,
    /// Current view mode
    pub view_mode: ViewMode,
    /// Update interval
    pub interval: Duration,
    /// Last update time
    pub last_update: Instant,
    /// Total updates
    pub update_count: u64,
    /// Should quit
    pub should_quit: bool,
    /// Paused (stop updates)
    pub paused: bool,
    /// Show detailed view for selected GPU (deprecated, use view_mode)
    pub show_detail: bool,
    /// Display settings (which metrics to show)
    pub settings: DisplaySettings,
    /// Selected setting in settings view
    pub settings_cursor: usize,
    /// System warnings (e.g., iomem not set)
    pub system_warning: Option<String>,
    /// Register access status
    pub register_status: Option<RegisterAccessStatus>,
}

impl App {
    pub fn new(interval: Duration) -> Self {
        Self::with_register_policy(interval, true)
    }

    pub fn with_register_policy(interval: Duration, enable_register_access: bool) -> Self {
        // Check register access status on startup
        let register_status = enable_register_access.then(RegisterAccessStatus::check);
        let system_warning = register_status.as_ref().and_then(RegisterAccessStatus::warning_message);
        
        Self {
            metrics: AllMetrics::default(),
            gpu_history: Vec::new(),
            selected_gpu: 0,
            show_help: false,
            view_mode: ViewMode::List,
            show_detail: false,
            interval,
            last_update: Instant::now(),
            update_count: 0,
            should_quit: false,
            paused: false,
            settings: DisplaySettings::default(),
            settings_cursor: 0,
            system_warning,
            register_status,
        }
    }

    /// Update metrics from collector
    pub fn update(&mut self, metrics: AllMetrics) {
        // Ensure we have history for all GPUs
        while self.gpu_history.len() < metrics.gpus.len() {
            self.gpu_history.push(GpuHistory::new());
        }

        // Update history for each GPU
        for (i, gpu) in metrics.gpus.iter().enumerate() {
            if i < self.gpu_history.len() {
                self.gpu_history[i].push(gpu);
            }
        }

        self.metrics = metrics;
        self.last_update = Instant::now();
        self.update_count += 1;

        // Clamp selected GPU
        if !self.metrics.gpus.is_empty() && self.selected_gpu >= self.metrics.gpus.len() {
            self.selected_gpu = self.metrics.gpus.len() - 1;
        }
    }

    /// Handle keyboard input
    pub fn handle_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        // Special handling for Settings view
        if self.view_mode == ViewMode::Settings {
            self.handle_settings_key(key, modifiers);
            return;
        }
        
        match key {
            KeyCode::Char('q') | KeyCode::Esc => {
                // Escape from views first, then quit
                if self.view_mode != ViewMode::List {
                    self.view_mode = ViewMode::List;
                    self.show_detail = false;
                } else if self.show_help {
                    self.show_help = false;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Char('?') | KeyCode::Char('h') => self.show_help = !self.show_help,
            KeyCode::Char(' ') | KeyCode::Char('P') => self.paused = !self.paused,
            KeyCode::Enter | KeyCode::Char('d') => {
                // Toggle between list and detail view
                self.view_mode = match self.view_mode {
                    ViewMode::List => ViewMode::Detail,
                    ViewMode::Detail => ViewMode::List,
                    ViewMode::Processes => ViewMode::Detail,
                    ViewMode::Settings => ViewMode::List,
                };
                self.show_detail = self.view_mode == ViewMode::Detail;
            }
            KeyCode::Char('p') => {
                // Toggle process view
                self.view_mode = match self.view_mode {
                    ViewMode::Processes => ViewMode::List,
                    _ => ViewMode::Processes,
                };
            }
            KeyCode::Char('s') | KeyCode::F(2) => {
                // Open settings (like htop F2)
                self.view_mode = ViewMode::Settings;
            }
            KeyCode::Char('1') => self.view_mode = ViewMode::List,
            KeyCode::Char('2') => self.view_mode = ViewMode::Detail,
            KeyCode::Char('3') => self.view_mode = ViewMode::Processes,
            KeyCode::Char('4') => self.view_mode = ViewMode::Settings,
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_gpu > 0 {
                    self.selected_gpu -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected_gpu + 1 < self.metrics.gpus.len() {
                    self.selected_gpu += 1;
                }
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.selected_gpu = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                if !self.metrics.gpus.is_empty() {
                    self.selected_gpu = self.metrics.gpus.len() - 1;
                }
            }
            KeyCode::Tab => {
                // Cycle through GPUs
                if !self.metrics.gpus.is_empty() {
                    self.selected_gpu = (self.selected_gpu + 1) % self.metrics.gpus.len();
                }
            }
            _ => {}
        }
    }
    
    /// Handle keyboard input in settings view
    fn handle_settings_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let settings_count = self.settings.as_list().len();
        
        match key {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::F(2) => {
                // Exit settings
                self.view_mode = ViewMode::List;
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.settings_cursor > 0 {
                    self.settings_cursor -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.settings_cursor + 1 < settings_count {
                    self.settings_cursor += 1;
                }
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.settings_cursor = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.settings_cursor = settings_count.saturating_sub(1);
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                // Toggle selected setting
                self.settings.toggle(self.settings_cursor);
            }
            KeyCode::Char('a') => {
                // Enable all
                self.settings.enable_all();
            }
            KeyCode::Char('n') => {
                // Disable all (none)
                self.settings.disable_all();
            }
            KeyCode::Char('1') => self.view_mode = ViewMode::List,
            KeyCode::Char('2') => self.view_mode = ViewMode::Detail,
            KeyCode::Char('3') => self.view_mode = ViewMode::Processes,
            _ => {}
        }
    }

    /// Get the currently selected GPU
    pub fn selected_gpu_metrics(&self) -> Option<&GpuMetrics> {
        self.metrics.gpus.get(self.selected_gpu)
    }

    /// Get history for selected GPU
    pub fn selected_gpu_history(&self) -> Option<&GpuHistory> {
        self.gpu_history.get(self.selected_gpu)
    }
}

/// Run the TUI application
pub async fn run_tui(interval: Duration, nvml_timeout: Duration, enable_register_access: bool, thermal_policy: crate::metrics::thermal::ThermalPolicy) -> anyhow::Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = App::with_register_policy(interval, enable_register_access);

    // Create collector
    let collector = MetricsCollector::with_thermal(nvml_timeout, enable_register_access, thermal_policy, interval);

    // Initial collection
    let metrics = collector.collect().await;
    app.update(metrics);

    // Main loop
    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    loop {
        // Draw UI
        terminal.draw(|f| ui::draw(f, &app))?;

        // Handle events with timeout
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key.code, key.modifiers);
            }
        }

        // Check if we should quit
        if app.should_quit {
            break;
        }

        // Update metrics if interval elapsed and not paused
        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();

            if !app.paused && app.last_update.elapsed() >= app.interval {
                let metrics = collector.collect().await;
                app.update(metrics);
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

#[cfg(test)]
mod thermal_policy_tests {
    use super::*;
    use crate::metrics::thermal::{ThermalBackend, ThermalMetadata, ThermalStatus};

    #[test]
    fn register_opt_out_skips_startup_probe_and_thermal_status_renders_without_gpu() {
        let mut app = App::with_register_policy(Duration::from_secs(10), false);
        assert!(app.register_status.is_none());
        assert!(app.system_warning.is_none());
        app.view_mode = ViewMode::Detail;
        app.metrics.gpus.push(GpuMetrics {
            name: "NVIDIA GeForce RTX 5090".into(),
            vram_temp: None,
            vram_thermal: Some(ThermalMetadata { backend: ThermalBackend::Rusd, status: ThermalStatus::Stale }),
            ..Default::default()
        });
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(140, 55)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &app)).unwrap();
        let text: String = terminal.backend().buffer().content().iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("rusd (stale)"));
        assert!(text.contains("N/A"));
    }

    #[test]
    fn host_auto_keeps_guest_temperature_and_valid_method_in_tui() {
        let manager=crate::metrics::thermal_worker::ThermalManager::with_command("/does/not/exist".into(),vec![]);
        let mut app=App::with_register_policy(Duration::from_secs(10),false);
        app.view_mode=ViewMode::Detail;
        app.metrics.gpus.push(GpuMetrics {name:"Guest RTX 5090".into(),source:crate::metrics::MetricSource::Vm,state:crate::state::GpuState::VmPassthrough,vram_temp:Some(72),..Default::default()});
        crate::metrics::thermal::ThermalPolicy {vram_backend:ThermalBackend::Auto,..Default::default()}.enrich(&mut app.metrics.gpus,&manager,app.interval);
        let mut terminal=Terminal::new(ratatui::backend::TestBackend::new(140,55)).unwrap();
        terminal.draw(|frame|ui::draw(frame,&app)).unwrap();
        let text:String=terminal.backend().buffer().content().iter().map(|cell|cell.symbol()).collect();
        assert!(text.contains("72°C"));assert!(text.contains("legacy (valid)"));
        assert_eq!(manager.max_live_children(),0);
    }
}
