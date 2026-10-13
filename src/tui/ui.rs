//! TUI Rendering
//!
//! Draws the nvtop-like interface using ratatui.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Gauge, List, ListItem, Paragraph,
        Sparkline, Wrap, Row, Table,
    },
    Frame,
};

use crate::metrics::{GpuMetrics, ProcessType};
use crate::state::GpuState;

use super::app::{App, DisplaySettings, GpuHistory, ViewMode};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Main draw function
pub fn draw(f: &mut Frame, app: &App) {
    let size = f.area();

    // Main layout: header, optional warning, GPU list, footer
    let has_warning = app.system_warning.is_some();
    
    let chunks = if has_warning {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Length(1), // Warning bar
                Constraint::Min(10),   // Main content
                Constraint::Length(1), // Footer
            ])
            .split(size)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(10),   // Main content
                Constraint::Length(1), // Footer
            ])
            .split(size)
    };

    draw_header(f, app, chunks[0]);
    
    let (content_area, footer_area) = if has_warning {
        draw_warning_bar(f, app, chunks[1]);
        (chunks[2], chunks[3])
    } else {
        (chunks[1], chunks[2])
    };
    
    match app.view_mode {
        ViewMode::List => draw_gpu_list(f, app, content_area),
        ViewMode::Detail => draw_detail_view(f, app, content_area),
        ViewMode::Processes => draw_process_view(f, app, content_area),
        ViewMode::Settings => draw_settings_view(f, app, content_area),
    }
    
    draw_footer(f, app, footer_area);

    // Help overlay
    if app.show_help {
        draw_help_overlay(f, size);
    }
}

/// Draw warning bar for system issues (e.g., iomem not set)
fn draw_warning_bar(f: &mut Frame, app: &App, area: Rect) {
    if let Some(ref warning) = app.system_warning {
        let warning_text = Paragraph::new(format!(" ⚠ {} ", warning))
            .style(Style::default().fg(Color::Black).bg(Color::Yellow));
        f.render_widget(warning_text, area);
    }
}

/// Draw header with title and status
fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let status = if app.paused {
        Span::styled(" [PAUSED]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" [RUNNING]", Style::default().fg(Color::Green))
    };

    let gpu_count = format!(
        " GPUs: {} NVML / {} PCIe",
        app.metrics.nvml_gpus, app.metrics.total_pci_gpus
    );
    
    let view_mode = match app.view_mode {
        ViewMode::List => "[1:List]",
        ViewMode::Detail => "[2:Detail]",
        ViewMode::Processes => "[3:Procs]",
        ViewMode::Settings => "[4:Setup]",
    };

    let title = Line::from(vec![
        Span::styled(
            format!(" DC Exporter v{}", VERSION),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        status,
        Span::raw(" │"),
        Span::styled(gpu_count, Style::default().fg(Color::White)),
        Span::raw(" │ "),
        Span::styled(view_mode, Style::default().fg(Color::Yellow)),
        Span::raw(" │ "),
        Span::styled(
            format!("Host: {}", app.metrics.hostname),
            Style::default().fg(Color::Gray),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);

    f.render_widget(block, area);
}

/// Draw GPU list view
fn draw_gpu_list(f: &mut Frame, app: &App, area: Rect) {
    if app.metrics.gpus.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" No GPUs Detected ");
        let msg = Paragraph::new("No NVIDIA GPUs found. Check drivers and permissions.")
            .block(block)
            .wrap(Wrap { trim: true });
        f.render_widget(msg, area);
        return;
    }

    // Calculate height per GPU (min 5 lines each for more info)
    let gpu_count = app.metrics.gpus.len();
    let height_per_gpu = ((area.height.saturating_sub(2)) as usize / gpu_count).max(5);

    let mut constraints: Vec<Constraint> = app
        .metrics
        .gpus
        .iter()
        .map(|_| Constraint::Length(height_per_gpu as u16))
        .collect();
    constraints.push(Constraint::Min(0)); // Fill remaining space

    let gpu_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    for (i, gpu) in app.metrics.gpus.iter().enumerate() {
        let is_selected = i == app.selected_gpu;
        let history = app.gpu_history.get(i);
        draw_gpu_card(f, gpu, history, gpu_chunks[i], is_selected);
    }
}

/// Draw a single GPU card
fn draw_gpu_card(
    f: &mut Frame,
    gpu: &GpuMetrics,
    history: Option<&GpuHistory>,
    area: Rect,
    selected: bool,
) {
    // VM metrics with guest agent show as blue, passthrough without metrics as magenta
    let is_vm_with_metrics = gpu.source == crate::metrics::MetricSource::Vm;
    
    let border_color = match gpu.state {
        GpuState::Ok => {
            if is_vm_with_metrics {
                if selected { Color::LightBlue } else { Color::Blue }
            } else if selected {
                Color::Cyan
            } else {
                Color::Green
            }
        }
        GpuState::Warning => Color::Yellow,
        GpuState::Error => Color::Red,
        GpuState::VmPassthrough => {
            if is_vm_with_metrics {
                Color::Blue  // Have VM metrics
            } else {
                Color::Magenta  // No metrics
            }
        }
        GpuState::Unavailable => Color::DarkGray,
        GpuState::HardwareFailure => Color::LightRed,
    };

    let state_str = match gpu.state {
        GpuState::Ok => "",
        GpuState::Warning => " ⚠ WARNING",
        GpuState::Error => " ✗ ERROR",
        GpuState::VmPassthrough => " ➜ VM",
        GpuState::Unavailable => " ✗ UNAVAIL",
        GpuState::HardwareFailure => " ✗ HW FAILURE",
    };

    // Show source indicator for VM metrics
    let source_str = match gpu.source {
        crate::metrics::MetricSource::Vm => " [via guest-agent]",
        _ => "",
    };

    let title = format!(
        " GPU {} │ {} │ {}{}{}",
        gpu.index, gpu.name, gpu.pci_bus_id, state_str, source_str
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            title,
            Style::default()
                .fg(if selected { Color::Cyan } else { Color::White })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Check if GPU is in VM but we have metrics from guest agent
    let is_vm_with_metrics = gpu.source == crate::metrics::MetricSource::Vm;
    
    if gpu.state == GpuState::VmPassthrough && !is_vm_with_metrics {
        let msg = Paragraph::new("GPU passed through to VM - no guest agent available")
            .style(Style::default().fg(Color::Magenta));
        f.render_widget(msg, inner);
        return;
    }

    // Layout inside card: temps/throttle | utilization bars | sparklines
    let card_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(32), // Temps + throttle
            Constraint::Length(32), // Utilization bars
            Constraint::Min(20),    // Sparklines
        ])
        .split(inner);

    // Temperature column with throttle indicators
    draw_temps_with_throttle(f, gpu, card_chunks[0]);

    // Utilization bars
    draw_utilization_bars(f, gpu, card_chunks[1]);

    // Sparklines (if we have history)
    if let Some(hist) = history {
        draw_sparklines(f, gpu, hist, card_chunks[2]);
    }
}

/// Draw temperature values with throttle indicators
fn draw_temps_with_throttle(f: &mut Frame, gpu: &GpuMetrics, area: Rect) {
    let gpu_temp = gpu.gpu_temp.map(|t| format!("{}°C", t)).unwrap_or_else(|| "N/A".to_string());
    let vram_temp = gpu.vram_temp.map(|t| format!("{}°C", t)).unwrap_or_else(|| "N/A".to_string());
    let hotspot_temp = gpu.hotspot_temp.map(|t| format!("{}°C", t)).unwrap_or_else(|| "N/A".to_string());
    let fan = gpu.fan_speed.map(|s| format!("{}%", s)).unwrap_or_else(|| "N/A".to_string());
    
    // Power with limit
    let power_str = match (gpu.power_usage, gpu.power_limit) {
        (Some(p), Some(l)) => format!("{:.0}/{:.0}W", p, l),
        (Some(p), None) => format!("{:.0}W", p),
        _ => "N/A".to_string(),
    };

    let temp_color = |temp: Option<u32>| match temp {
        Some(t) if t >= 85 => Color::Red,
        Some(t) if t >= 75 => Color::Yellow,
        Some(_) => Color::Green,
        None => Color::DarkGray,
    };

    // Build throttle indicators
    let mut throttle_indicators = Vec::new();
    let tr = &gpu.throttle_reasons;
    if tr.hw_thermal_slowdown || tr.sw_thermal_slowdown {
        throttle_indicators.push(Span::styled("🔥", Style::default().fg(Color::Red)));
    }
    if tr.sw_power_cap || tr.hw_power_brake_slowdown {
        throttle_indicators.push(Span::styled("⚡", Style::default().fg(Color::Yellow)));
    }
    if tr.gpu_idle {
        throttle_indicators.push(Span::styled("⏸", Style::default().fg(Color::Gray)));
    }
    if tr.hw_slowdown {
        throttle_indicators.push(Span::styled("⬇", Style::default().fg(Color::Magenta)));
    }
    
    let throttle_line = if throttle_indicators.is_empty() {
        Line::from(vec![Span::styled("  No throttling", Style::default().fg(Color::DarkGray))])
    } else {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(throttle_indicators);
        Line::from(spans)
    };

    let lines = vec![
        Line::from(vec![
            Span::raw("GPU:     "),
            Span::styled(format!("{:>5}", gpu_temp), Style::default().fg(temp_color(gpu.gpu_temp))),
            Span::raw("  Fan: "),
            Span::styled(fan, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("VRAM:    ", Style::default().fg(Color::Yellow)),
            Span::styled(format!("{:>5}", vram_temp), Style::default().fg(temp_color(gpu.vram_temp)).add_modifier(Modifier::BOLD)),
            Span::raw("  Pwr: "),
            Span::styled(power_str, Style::default().fg(Color::Magenta)),
        ]),
        Line::from(vec![
            Span::styled("Hotspot: ", Style::default().fg(Color::Red)),
            Span::styled(format!("{:>5}", hotspot_temp), Style::default().fg(temp_color(gpu.hotspot_temp)).add_modifier(Modifier::BOLD)),
        ]),
        throttle_line,
    ];

    let para = Paragraph::new(lines);
    f.render_widget(para, area);
}

/// Draw utilization bars (GPU util, memory util, memory usage, encoder/decoder)
fn draw_utilization_bars(f: &mut Frame, gpu: &GpuMetrics, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    // GPU Utilization
    let gpu_util = gpu.gpu_util.unwrap_or(0);
    let gpu_gauge = Gauge::default()
        .label(format!("GPU: {:>3}%", gpu_util))
        .ratio(gpu_util as f64 / 100.0)
        .gauge_style(Style::default().fg(util_color(gpu_util)).bg(Color::DarkGray));
    f.render_widget(gpu_gauge, chunks[0]);

    // Memory Utilization
    let mem_util = gpu.mem_util.unwrap_or(0);
    let mem_gauge = Gauge::default()
        .label(format!("MEM: {:>3}%", mem_util))
        .ratio(mem_util as f64 / 100.0)
        .gauge_style(Style::default().fg(util_color(mem_util)).bg(Color::DarkGray));
    f.render_widget(mem_gauge, chunks[1]);

    // Memory Usage (absolute)
    let (used, total) = (gpu.fb_used.unwrap_or(0), gpu.fb_total.unwrap_or(0));
    let ratio = if total > 0 { used as f64 / total as f64 } else { 0.0 };
    let mem_label = format!("VRAM: {}/{}MB", used, total);
    let vram_gauge = Gauge::default()
        .label(mem_label)
        .ratio(ratio)
        .gauge_style(Style::default().fg(Color::Blue).bg(Color::DarkGray));
    f.render_widget(vram_gauge, chunks[2]);
    
    // Encoder/Decoder or NVLink
    if gpu.nvlink_active > 0 {
        let nvlink_label = format!("NVLink: {} active", gpu.nvlink_active);
        let nvlink_gauge = Gauge::default()
            .label(nvlink_label)
            .ratio(1.0)  // Full bar when NVLink is active
            .gauge_style(Style::default().fg(Color::LightGreen).bg(Color::DarkGray));
        f.render_widget(nvlink_gauge, chunks[3]);
    } else {
        let enc = gpu.encoder_util.unwrap_or(0);
        let dec = gpu.decoder_util.unwrap_or(0);
        let enc_dec_label = format!("ENC:{:>2}% DEC:{:>2}%", enc, dec);
        let enc_gauge = Gauge::default()
            .label(enc_dec_label)
            .ratio(enc as f64 / 100.0)
            .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray));
        f.render_widget(enc_gauge, chunks[3]);
    }
}

/// Draw sparklines for history with annotations
fn draw_sparklines(f: &mut Frame, gpu: &GpuMetrics, history: &GpuHistory, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    // GPU Temp history with current value
    let gpu_data: Vec<u64> = history.gpu_temp.iter().copied().collect();
    let _current_temp = gpu.gpu_temp.unwrap_or(0);
    let gpu_spark = Sparkline::default()
        .data(&gpu_data)
        .max(100)
        .style(Style::default().fg(Color::Green));
    f.render_widget(gpu_spark, chunks[0]);

    // GPU Util history
    let util_data: Vec<u64> = history.gpu_util.iter().copied().collect();
    let util_spark = Sparkline::default()
        .data(&util_data)
        .max(100)
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(util_spark, chunks[1]);

    // Power history
    let power_data: Vec<u64> = history.power.iter().copied().collect();
    let power_spark = Sparkline::default()
        .data(&power_data)
        .max(gpu.power_limit.unwrap_or(500.0) as u64)
        .style(Style::default().fg(Color::Magenta));
    f.render_widget(power_spark, chunks[2]);
    
    // PCIe or NVLink throughput
    if gpu.nvlink_active > 0 {
        let nvlink_data: Vec<u64> = history.nvlink_tx.iter().copied().collect();
        let nvlink_spark = Sparkline::default()
            .data(&nvlink_data)
            .style(Style::default().fg(Color::LightGreen));
        f.render_widget(nvlink_spark, chunks[3]);
    } else {
        let pcie_data: Vec<u64> = history.pcie_tx.iter().copied().collect();
        let pcie_spark = Sparkline::default()
            .data(&pcie_data)
            .style(Style::default().fg(Color::White));
        f.render_widget(pcie_spark, chunks[3]);
    }
}

/// Draw detailed view for selected GPU
fn draw_detail_view(f: &mut Frame, app: &App, area: Rect) {
    let gpu = match app.selected_gpu_metrics() {
        Some(g) => g,
        None => {
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" No GPU Selected ");
            f.render_widget(block, area);
            return;
        }
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" GPU {} Detail │ {} ", gpu.index, gpu.name));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split into left (info) and right (graphs)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(inner);

    // Left: detailed info in two columns
    draw_detail_info(f, gpu, &app.settings, chunks[0]);

    // Right: history graphs (if enabled)
    if app.settings.show_graphs {
        if let Some(history) = app.selected_gpu_history() {
            draw_detail_graphs(f, gpu, history, &app.settings, chunks[1]);
        }
    }
}

/// Draw detailed GPU info panel
fn draw_detail_info(f: &mut Frame, gpu: &GpuMetrics, settings: &DisplaySettings, area: Rect) {
    let mut info_items = vec![
        ListItem::new(Line::from(vec![
            Span::styled("UUID: ", Style::default().fg(Color::Gray)),
            Span::raw(&gpu.uuid),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("PCI Bus: ", Style::default().fg(Color::Gray)),
            Span::raw(&gpu.pci_bus_id),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Driver: ", Style::default().fg(Color::Gray)),
            Span::raw(&gpu.driver_version),
        ])),
    ];
    
    // Temperatures (if any temp setting enabled)
    if settings.show_gpu_temp || settings.show_vram_temp || settings.show_hotspot_temp {
        info_items.push(ListItem::new(""));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("─── Temperatures ───", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ])));
        
        if settings.show_gpu_temp {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("GPU: ", Style::default().fg(Color::Green)),
                Span::raw(gpu.gpu_temp.map(|t| format!("{}°C", t)).unwrap_or_else(|| "N/A".to_string())),
            ])));
        }
        if settings.show_vram_temp {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("VRAM: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(gpu.vram_temp.map(|t| format!("{}°C", t)).unwrap_or_else(|| "N/A".to_string())),
                if gpu.vram_temp_approximated { Span::raw(" (approx)") } else { Span::raw("") },
            ])));
            let valid = crate::metrics::thermal::measured_vram_supported(gpu);
            let (backend, status) = gpu.vram_thermal.as_ref()
                .map(|m| (m.backend.as_str(), m.status.as_str()))
                .unwrap_or(("legacy", if valid { "valid" } else { "unsupported" }));
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("VRAM method: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{} ({})", backend, status)),
            ])));
        }
        if settings.show_hotspot_temp {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Hotspot: ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw(gpu.hotspot_temp.map(|t| format!("{}°C", t)).unwrap_or_else(|| "N/A".to_string())),
                if gpu.hotspot_temp_approximated { Span::raw(" (approx)") } else { Span::raw("") },
            ])));
        }
    }
    
    // Power
    if settings.show_power || settings.show_power_limit {
        info_items.push(ListItem::new(""));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("─── Power ───", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
        ])));
        
        if settings.show_power {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Usage: ", Style::default().fg(Color::Magenta)),
                Span::raw(gpu.power_usage.map(|p| format!("{:.1}W", p)).unwrap_or_else(|| "N/A".to_string())),
            ])));
        }
        if settings.show_power_limit {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Limit: ", Style::default().fg(Color::Gray)),
                Span::raw(gpu.power_limit.map(|p| format!("{:.0}W", p)).unwrap_or_else(|| "N/A".to_string())),
            ])));
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Max: ", Style::default().fg(Color::Gray)),
                Span::raw(gpu.power_max.map(|p| format!("{:.0}W", p)).unwrap_or_else(|| "N/A".to_string())),
            ])));
        }
    }
    
    // Clocks
    if settings.show_clocks || settings.show_fan {
        info_items.push(ListItem::new(""));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("─── Clocks ───", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ])));
        
        if settings.show_clocks {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("SM: ", Style::default().fg(Color::Cyan)),
                Span::raw(gpu.sm_clock.map(|c| format!("{} MHz", c)).unwrap_or_else(|| "N/A".to_string())),
            ])));
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Memory: ", Style::default().fg(Color::Cyan)),
                Span::raw(gpu.mem_clock.map(|c| format!("{} MHz", c)).unwrap_or_else(|| "N/A".to_string())),
            ])));
        }
        if settings.show_fan {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Fan: ", Style::default().fg(Color::Cyan)),
                Span::raw(gpu.fan_speed.map(|s| format!("{}%", s)).unwrap_or_else(|| "N/A".to_string())),
            ])));
        }
    }
    
    // Utilization
    if settings.show_gpu_util || settings.show_mem_util || settings.show_encoder || settings.show_decoder {
        info_items.push(ListItem::new(""));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("─── Utilization ───", Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
        ])));
        
        if settings.show_gpu_util {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("GPU: ", Style::default().fg(Color::Blue)),
                Span::raw(gpu.gpu_util.map(|u| format!("{}%", u)).unwrap_or_else(|| "N/A".to_string())),
            ])));
        }
        if settings.show_mem_util {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Memory: ", Style::default().fg(Color::Blue)),
                Span::raw(gpu.mem_util.map(|u| format!("{}%", u)).unwrap_or_else(|| "N/A".to_string())),
            ])));
        }
        if settings.show_encoder {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Encoder: ", Style::default().fg(Color::Blue)),
                Span::raw(gpu.encoder_util.map(|u| format!("{}%", u)).unwrap_or_else(|| "N/A".to_string())),
            ])));
        }
        if settings.show_decoder {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Decoder: ", Style::default().fg(Color::Blue)),
                Span::raw(gpu.decoder_util.map(|u| format!("{}%", u)).unwrap_or_else(|| "N/A".to_string())),
            ])));
        }
    }
    
    // Memory
    if settings.show_vram_usage {
        info_items.push(ListItem::new(""));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("─── VRAM ───", Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD)),
        ])));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("Used: ", Style::default().fg(Color::LightBlue)),
            Span::raw(gpu.fb_used.map(|m| format!("{} MiB", m)).unwrap_or_else(|| "N/A".to_string())),
        ])));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("Free: ", Style::default().fg(Color::LightBlue)),
            Span::raw(gpu.fb_free.map(|m| format!("{} MiB", m)).unwrap_or_else(|| "N/A".to_string())),
        ])));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("Total: ", Style::default().fg(Color::LightBlue)),
            Span::raw(gpu.fb_total.map(|m| format!("{} MiB", m)).unwrap_or_else(|| "N/A".to_string())),
        ])));
    }
    
    // PCIe info
    if settings.show_pcie_info || settings.show_pcie_throughput {
        info_items.push(ListItem::new(""));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("─── PCIe ───", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ])));
        
        if settings.show_pcie_info {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Gen: ", Style::default().fg(Color::White)),
                Span::raw(format!(
                    "{}/{}",
                    gpu.pcie_gen.map(|g| g.to_string()).unwrap_or_else(|| "?".to_string()),
                    gpu.pcie_gen_max.map(|g| g.to_string()).unwrap_or_else(|| "?".to_string())
                )),
                Span::raw(" x"),
                Span::raw(format!(
                    "{}/{}",
                    gpu.pcie_width.map(|w| w.to_string()).unwrap_or_else(|| "?".to_string()),
                    gpu.pcie_width_max.map(|w| w.to_string()).unwrap_or_else(|| "?".to_string())
                )),
            ])));
        }
        
        if settings.show_pcie_throughput {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("TX: ", Style::default().fg(Color::White)),
                Span::raw(format_throughput(gpu.pcie_tx_throughput)),
                Span::raw("  RX: "),
                Span::raw(format_throughput(gpu.pcie_rx_throughput)),
            ])));
        }
    }
    
    // NVLink info (if available and enabled)
    if settings.show_nvlink && gpu.nvlink_active > 0 {
        info_items.push(ListItem::new(""));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("─── NVLink ───", Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD)),
        ])));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("Links: ", Style::default().fg(Color::LightGreen)),
            Span::raw(format!("{} active", gpu.nvlink_active)),
        ])));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("TX: ", Style::default().fg(Color::LightGreen)),
            Span::raw(format_throughput(gpu.nvlink_tx_throughput)),
            Span::raw("  RX: "),
            Span::raw(format_throughput(gpu.nvlink_rx_throughput)),
        ])));
        if gpu.nvlink_errors > 0 {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Errors: ", Style::default().fg(Color::Red)),
                Span::styled(format!("{}", gpu.nvlink_errors), Style::default().fg(Color::Red)),
            ])));
        }
    }
    
    // ECC info (if enabled in both GPU and settings)
    if settings.show_ecc && gpu.ecc_enabled {
        info_items.push(ListItem::new(""));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("─── ECC ───", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ])));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("SBE: ", Style::default().fg(Color::Yellow)),
            Span::raw(gpu.ecc_sbe.map(|e| e.to_string()).unwrap_or_else(|| "0".to_string())),
            Span::raw(" (correctable)"),
        ])));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("DBE: ", Style::default().fg(if gpu.ecc_dbe.unwrap_or(0) > 0 { Color::Red } else { Color::Yellow })),
            Span::raw(gpu.ecc_dbe.map(|e| e.to_string()).unwrap_or_else(|| "0".to_string())),
            Span::raw(" (uncorrectable)"),
        ])));
    }
    
    // Throttle reasons
    if settings.show_throttle {
        info_items.push(ListItem::new(""));
        info_items.push(ListItem::new(Line::from(vec![
            Span::styled("─── Throttle Reasons ───", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        ])));
        
        let tr = &gpu.throttle_reasons;
        let mut throttle_active = false;
        if tr.hw_thermal_slowdown {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("🔥 HW Thermal Slowdown", Style::default().fg(Color::Red)),
            ])));
            throttle_active = true;
        }
        if tr.sw_thermal_slowdown {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("🔥 SW Thermal Slowdown", Style::default().fg(Color::Red)),
            ])));
            throttle_active = true;
        }
        if tr.sw_power_cap {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("⚡ SW Power Cap", Style::default().fg(Color::Yellow)),
            ])));
            throttle_active = true;
        }
        if tr.hw_power_brake_slowdown {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("⚡ HW Power Brake", Style::default().fg(Color::Yellow)),
            ])));
            throttle_active = true;
        }
        if tr.hw_slowdown {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("⬇ HW Slowdown", Style::default().fg(Color::Magenta)),
            ])));
            throttle_active = true;
        }
        if tr.gpu_idle {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("⏸ GPU Idle", Style::default().fg(Color::Gray)),
            ])));
            throttle_active = true;
        }
        if tr.applications_clocks_setting {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("⚙ App Clocks Setting", Style::default().fg(Color::Gray)),
            ])));
            throttle_active = true;
        }
        if !throttle_active {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("None", Style::default().fg(Color::Green)),
            ])));
        }
    }
    
    // State and errors
    if settings.show_state || settings.show_aer_errors || settings.show_process_count {
        info_items.push(ListItem::new(""));
        
        if settings.show_state {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("State: ", Style::default().fg(Color::White)),
                Span::styled(
                    format!("{:?}", gpu.state),
                    Style::default().fg(state_color(gpu.state)),
                ),
            ])));
        }
        
        if settings.show_aer_errors {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("AER Errors: ", Style::default().fg(if gpu.aer_errors > 0 { Color::Red } else { Color::Gray })),
                Span::raw(format!("{}", gpu.aer_errors)),
            ])));
        }
        
        if settings.show_process_count {
            info_items.push(ListItem::new(Line::from(vec![
                Span::styled("Processes: ", Style::default().fg(Color::Gray)),
                Span::raw(format!("{} compute, {} graphics", gpu.compute_processes, gpu.graphics_processes)),
            ])));
        }
    }

    let info_list = List::new(info_items)
        .block(Block::default().borders(Borders::RIGHT).title(" Info "));
    f.render_widget(info_list, area);
}

/// Draw detailed graphs for selected GPU
fn draw_detail_graphs(f: &mut Frame, gpu: &GpuMetrics, history: &GpuHistory, settings: &DisplaySettings, area: Rect) {
    // Determine how many graph sections we need
    let has_nvlink = gpu.nvlink_active > 0 && settings.show_nvlink;
    
    let chunks = if has_nvlink {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(33),
                Constraint::Percentage(33),
                Constraint::Percentage(34),
            ])
            .split(area)
    };

    // Temperature graph with stats
    let (temp_min, temp_max, temp_avg) = GpuHistory::stats(&history.gpu_temp);
    let title = if settings.show_graph_stats {
        format!(" Temperature (min:{} max:{} avg:{})°C ", temp_min, temp_max, temp_avg)
    } else {
        " Temperature ".to_string()
    };
    let temp_block = Block::default()
        .borders(Borders::ALL)
        .title(title);
    let temp_inner = temp_block.inner(chunks[0]);
    f.render_widget(temp_block, chunks[0]);

    let temp_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(temp_inner);

    // GPU temp sparkline
    let gpu_data: Vec<u64> = history.gpu_temp.iter().copied().collect();
    f.render_widget(
        Sparkline::default()
            .data(&gpu_data)
            .max(100)
            .style(Style::default().fg(Color::Green)),
        temp_chunks[0],
    );

    // VRAM temp sparkline
    let vram_data: Vec<u64> = history.vram_temp.iter().copied().collect();
    f.render_widget(
        Sparkline::default()
            .data(&vram_data)
            .max(100)
            .style(Style::default().fg(Color::Yellow)),
        temp_chunks[1],
    );

    // Hotspot temp sparkline
    let hotspot_data: Vec<u64> = history.hotspot_temp.iter().copied().collect();
    f.render_widget(
        Sparkline::default()
            .data(&hotspot_data)
            .max(100)
            .style(Style::default().fg(Color::Red)),
        temp_chunks[2],
    );

    // Utilization graph with stats
    let (util_min, util_max, util_avg) = GpuHistory::stats(&history.gpu_util);
    let title = if settings.show_graph_stats {
        format!(" Utilization (min:{} max:{} avg:{})% ", util_min, util_max, util_avg)
    } else {
        " Utilization ".to_string()
    };
    let util_block = Block::default()
        .borders(Borders::ALL)
        .title(title);
    let util_inner = util_block.inner(chunks[1]);
    f.render_widget(util_block, chunks[1]);

    let util_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)])
        .split(util_inner);

    let gpu_util_data: Vec<u64> = history.gpu_util.iter().copied().collect();
    f.render_widget(
        Sparkline::default()
            .data(&gpu_util_data)
            .max(100)
            .style(Style::default().fg(Color::Cyan)),
        util_chunks[0],
    );

    let mem_util_data: Vec<u64> = history.mem_util.iter().copied().collect();
    f.render_widget(
        Sparkline::default()
            .data(&mem_util_data)
            .max(100)
            .style(Style::default().fg(Color::Blue)),
        util_chunks[1],
    );
    
    // Encoder/Decoder
    let enc_data: Vec<u64> = history.encoder_util.iter().copied().collect();
    f.render_widget(
        Sparkline::default()
            .data(&enc_data)
            .max(100)
            .style(Style::default().fg(Color::LightCyan)),
        util_chunks[2],
    );

    // Power graph with stats
    let (pwr_min, pwr_max, pwr_avg) = GpuHistory::stats(&history.power);
    let title = if settings.show_graph_stats {
        format!(" Power (min:{} max:{} avg:{})W ", pwr_min, pwr_max, pwr_avg)
    } else {
        " Power ".to_string()
    };
    let power_block = Block::default()
        .borders(Borders::ALL)
        .title(title);
    let power_inner = power_block.inner(chunks[2]);
    f.render_widget(power_block, chunks[2]);

    let power_data: Vec<u64> = history.power.iter().copied().collect();
    f.render_widget(
        Sparkline::default()
            .data(&power_data)
            .max(gpu.power_limit.unwrap_or(500.0) as u64)
            .style(Style::default().fg(Color::Magenta)),
        power_inner,
    );
    
    // NVLink graph (if applicable)
    if has_nvlink && chunks.len() > 3 {
        let nvlink_block = Block::default()
            .borders(Borders::ALL)
            .title(" NVLink Throughput (MB/s) ");
        let nvlink_inner = nvlink_block.inner(chunks[3]);
        f.render_widget(nvlink_block, chunks[3]);
        
        let nvlink_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(nvlink_inner);
        
        let nvlink_tx_data: Vec<u64> = history.nvlink_tx.iter().copied().collect();
        f.render_widget(
            Sparkline::default()
                .data(&nvlink_tx_data)
                .style(Style::default().fg(Color::LightGreen)),
            nvlink_chunks[0],
        );
        
        let nvlink_rx_data: Vec<u64> = history.nvlink_rx.iter().copied().collect();
        f.render_widget(
            Sparkline::default()
                .data(&nvlink_rx_data)
                .style(Style::default().fg(Color::Green)),
            nvlink_chunks[1],
        );
    }
}

/// Draw process view
fn draw_process_view(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" GPU Processes ");
    
    let inner = block.inner(area);
    f.render_widget(block, area);
    
    // Collect all processes from all GPUs
    let mut all_processes: Vec<(u32, &str, &crate::metrics::GpuProcess)> = Vec::new();
    
    for gpu in &app.metrics.gpus {
        for proc in &gpu.process_list {
            all_processes.push((gpu.index, &gpu.name, proc));
        }
    }
    
    if all_processes.is_empty() {
        let msg = Paragraph::new("No GPU processes running")
            .style(Style::default().fg(Color::Gray));
        f.render_widget(msg, inner);
        return;
    }
    
    // Sort by memory usage
    all_processes.sort_by(|a, b| b.2.used_memory.cmp(&a.2.used_memory));
    
    // Create table
    let header = Row::new(vec!["GPU", "PID", "Name", "Type", "Memory"])
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    
    let rows: Vec<Row> = all_processes
        .iter()
        .map(|(gpu_idx, _gpu_name, proc)| {
            let type_str = match proc.process_type {
                ProcessType::Compute => "Compute",
                ProcessType::Graphics => "Graphics",
                ProcessType::Mixed => "Mixed",
            };
            let type_color = match proc.process_type {
                ProcessType::Compute => Color::Cyan,
                ProcessType::Graphics => Color::Magenta,
                ProcessType::Mixed => Color::Yellow,
            };
            
            Row::new(vec![
                format!("{}", gpu_idx),
                format!("{}", proc.pid),
                proc.name.clone(),
                type_str.to_string(),
                format!("{} MiB", proc.used_memory),
            ])
            .style(Style::default().fg(type_color))
        })
        .collect();
    
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(Block::default());
    
    f.render_widget(table, inner);
}

/// Draw settings view (like htop F2 setup)
fn draw_settings_view(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Setup - Display Metrics (F2/s to exit) ");
    
    let inner = block.inner(area);
    f.render_widget(block, area);
    
    // Split into left (settings list) and right (preview/info)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(inner);
    
    // Settings list
    let settings_list = app.settings.as_list();
    let items: Vec<ListItem> = settings_list
        .iter()
        .enumerate()
        .map(|(i, (name, enabled, _desc))| {
            let checkbox = if *enabled { "[✓]" } else { "[ ]" };
            let style = if i == app.settings_cursor {
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else if *enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", checkbox), style),
                Span::styled(*name, style),
            ]))
        })
        .collect();
    
    let list = List::new(items)
        .block(Block::default().borders(Borders::RIGHT).title(" Metrics "));
    f.render_widget(list, chunks[0]);
    
    // Info panel
    let enabled_count = app.settings.enabled_count();
    let total_count = settings_list.len();
    
    // Get description of selected setting
    let (selected_name, selected_enabled, selected_desc) = settings_list
        .get(app.settings_cursor)
        .copied()
        .unwrap_or(("", false, ""));
    
    let info_lines = vec![
        Line::from(""),
        Line::from(Span::styled(" Selected:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(format!("   {}", selected_name)),
        Line::from(""),
        Line::from(Span::styled(" Description:", Style::default().fg(Color::Gray))),
        Line::from(format!("   {}", selected_desc)),
        Line::from(""),
        Line::from(Span::styled(" Status:", Style::default().fg(Color::Gray))),
        Line::from(format!("   {}", if selected_enabled { "Enabled" } else { "Disabled" })),
        Line::from(""),
        Line::from("─".repeat(30)),
        Line::from(""),
        Line::from(Span::styled(" Statistics:", Style::default().fg(Color::Cyan))),
        Line::from(format!("   {}/{} metrics enabled", enabled_count, total_count)),
        Line::from(""),
        Line::from("─".repeat(30)),
        Line::from(""),
        Line::from(Span::styled(" Controls:", Style::default().fg(Color::Yellow))),
        Line::from("   ↑/↓ k/j  Navigate"),
        Line::from("   Space    Toggle metric"),
        Line::from("   a        Enable all"),
        Line::from("   n        Disable all"),
        Line::from("   Esc/F2   Exit settings"),
    ];
    
    let info = Paragraph::new(info_lines)
        .block(Block::default().borders(Borders::NONE).title(" Info "));
    f.render_widget(info, chunks[1]);
}

/// Draw help overlay
fn draw_help_overlay(f: &mut Frame, area: Rect) {
    let help_width = 55;
    let help_height = 26;
    let x = (area.width.saturating_sub(help_width)) / 2;
    let y = (area.height.saturating_sub(help_height)) / 2;
    let help_area = Rect::new(x, y, help_width, help_height);

    // Clear background
    f.render_widget(Clear, help_area);

    let help_text = vec![
        Line::from(Span::styled(
            " DC Exporter TUI - Keyboard Shortcuts ",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(" Navigation:"),
        Line::from("   ↑/k, ↓/j    - Select GPU"),
        Line::from("   Tab         - Cycle GPUs"),
        Line::from("   g/G         - First/Last GPU"),
        Line::from(""),
        Line::from(" Views:"),
        Line::from("   1           - List view (default)"),
        Line::from("   2 / Enter/d - Detail view"),
        Line::from("   3 / p       - Process view"),
        Line::from("   4 / s / F2  - Setup (enable/disable metrics)"),
        Line::from(""),
        Line::from(" Controls:"),
        Line::from("   Space/P     - Pause/Resume updates"),
        Line::from("   ?/h         - Toggle this help"),
        Line::from(""),
        Line::from(" Throttle Icons:"),
        Line::from("   🔥 Thermal  ⚡ Power  ⏸ Idle  ⬇ HW Slowdown"),
        Line::from(""),
        Line::from(" Exit:"),
        Line::from("   q/Esc       - Back / Quit"),
        Line::from("   Ctrl+C      - Force quit"),
    ];

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Help "),
        )
        .style(Style::default().fg(Color::White).bg(Color::Black));

    f.render_widget(help, help_area);
}

/// Draw footer with status
fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let elapsed = app.last_update.elapsed().as_secs();
    let view_hint = match app.view_mode {
        ViewMode::List => "Enter=Detail p=Procs s=Setup",
        ViewMode::Detail => "Esc=Back p=Procs s=Setup",
        ViewMode::Processes => "Esc=Back d=Detail s=Setup",
        ViewMode::Settings => "Esc=Back Space=Toggle a=All n=None",
    };
    
    let status = if app.paused {
        format!("PAUSED │ {} │ Press Space to resume", view_hint)
    } else {
        format!(
            "{}s ago │ {} │ {}s",
            elapsed, view_hint, app.interval.as_secs()
        )
    };

    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" [?] Help ", Style::default().fg(Color::Yellow)),
        Span::styled(" [F2] Setup ", Style::default().fg(Color::Cyan)),
        Span::raw("│ "),
        Span::styled(status, Style::default().fg(Color::Gray)),
    ]))
    .style(Style::default().fg(Color::DarkGray));

    f.render_widget(footer, area);
}

/// Format throughput in human-readable form
fn format_throughput(kb_per_sec: Option<u64>) -> String {
    match kb_per_sec {
        Some(kb) if kb >= 1024 * 1024 => format!("{:.1} GB/s", kb as f64 / (1024.0 * 1024.0)),
        Some(kb) if kb >= 1024 => format!("{:.1} MB/s", kb as f64 / 1024.0),
        Some(kb) => format!("{} KB/s", kb),
        None => "N/A".to_string(),
    }
}

/// Get color based on utilization percentage
fn util_color(util: u32) -> Color {
    match util {
        0..=50 => Color::Green,
        51..=80 => Color::Yellow,
        _ => Color::Red,
    }
}

/// Get color based on GPU state
fn state_color(state: GpuState) -> Color {
    match state {
        GpuState::Ok => Color::Green,
        GpuState::Warning => Color::Yellow,
        GpuState::Error => Color::Red,
        GpuState::VmPassthrough => Color::Magenta,
        GpuState::Unavailable => Color::DarkGray,
        GpuState::HardwareFailure => Color::LightRed,
    }
}
