//! DC Exporter - GPU Metrics Exporter for Prometheus
//!
//! Provides unique metrics not available in dcgm-exporter:
//! - VRAM Temperature (via direct register access)
//! - Hotspot Temperature (via direct register access)
//! - Throttle Reasons
//! - AER/PCIe Errors
//!
//! Key Safety Features:
//! - Never blocks GPU passthrough (one-shot mode, timeout-protected calls)
//! - Graceful degradation on driver crashes (outputs error states)
//! - Non-blocking PCIe detection even when NVML fails
//!
//! Modes:
//! - HTTP server (default): Prometheus metrics endpoint
//! - TUI mode (--tui): Interactive terminal UI like nvtop
//! - One-shot mode (--one-shot): Collect once and exit

mod config;
mod metrics;
mod safe_nvml;
mod state;
mod tui;

use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{RwLock, Mutex};
use tracing::{info, warn, debug};

use crate::config::Config;
use crate::metrics::collector::MetricsCollector;
use crate::metrics::prometheus::PrometheusFormatter;
use crate::metrics::thermal::ThermalBackend;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "dc-exporter")]
#[command(author = "CryptoLabs ZA")]
#[command(version = VERSION)]
#[command(about = "GPU metrics exporter for Prometheus (VRAM temp, hotspot, throttle reasons)")]
#[command(long_about = "DC Exporter - GPU Metrics Tool\n\n\
    Modes:\n  \
    - HTTP server (default): Serves Prometheus metrics on --port\n  \
    - TUI mode (--tui): Interactive terminal UI like nvtop\n  \
    - One-shot (--one-shot): Collect once and exit\n\n\
    Unique metrics not in dcgm-exporter:\n  \
    - VRAM Temperature\n  \
    - Hotspot/Junction Temperature\n  \
    - Throttle Reasons\n  \
    - AER/PCIe Errors")]
struct Args {
    /// Configuration file path
    #[arg(short, long, default_value = "/etc/dc-exporter/config.toml")]
    config: PathBuf,

    /// HTTP server port
    #[arg(short, long, default_value = "9835")]
    port: u16,

    /// Collection interval in seconds
    #[arg(short, long, default_value = "10")]
    interval: u64,

    /// One-shot mode: collect once and exit
    #[arg(long)]
    one_shot: bool,

    /// TUI mode: interactive terminal UI (like nvtop)
    #[arg(long, short = 't')]
    tui: bool,

    /// Debug mode: verbose logging
    #[arg(short, long)]
    debug: bool,

    /// Timeout for NVML operations (seconds)
    #[arg(long, default_value = "5")]
    nvml_timeout: u64,

    /// Disable direct register access (VRAM/hotspot temps)
    #[arg(long)]
    no_register_access: bool,

    /// VRAM thermal method (default: legacy; auto/rusd remain experimental)
    #[arg(long, value_enum)]
    vram_backend: Option<ThermalBackend>,

    /// Deadline for optional thermal collection in milliseconds (default: 2000)
    #[arg(long)]
    thermal_timeout_ms: Option<u64>,

    /// Maximum current thermal sample age in milliseconds (default: max(3*interval, deadline))
    #[arg(long)]
    thermal_max_sample_age_ms: Option<u64>,

    #[arg(long, hide = true)]
    thermal_helper: bool,
}

/// Rate limiter configuration
const MAX_REQUESTS_PER_SECOND: u64 = 10;  // Per IP
const RATE_LIMIT_WINDOW_MS: u64 = 1000;
const MAX_CONCURRENT_CONNECTIONS: usize = 100;

/// Simple per-IP rate limiter
struct RateLimiter {
    /// Map of IP -> (request_count, window_start_ms)
    requests: Mutex<HashMap<IpAddr, (u64, u64)>>,
    /// Total active connections
    active_connections: AtomicU64,
}

impl RateLimiter {
    fn new() -> Self {
        RateLimiter {
            requests: Mutex::new(HashMap::new()),
            active_connections: AtomicU64::new(0),
        }
    }
    
    /// Check if request should be allowed (returns true if allowed)
    async fn check(&self, ip: IpAddr) -> bool {
        // Check total connections first
        let active = self.active_connections.load(Ordering::Relaxed);
        if active >= MAX_CONCURRENT_CONNECTIONS as u64 {
            debug!("Rate limit: too many connections ({})", active);
            return false;
        }
        
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        
        let mut requests = self.requests.lock().await;
        
        // Clean up old entries (older than 10 seconds)
        requests.retain(|_, (_, window_start)| now_ms - *window_start < 10_000);
        
        let entry = requests.entry(ip).or_insert((0, now_ms));
        
        // Check if we're in a new window
        if now_ms - entry.1 >= RATE_LIMIT_WINDOW_MS {
            // New window
            entry.0 = 1;
            entry.1 = now_ms;
            true
        } else if entry.0 < MAX_REQUESTS_PER_SECOND {
            // Within limit
            entry.0 += 1;
            true
        } else {
            // Rate limited
            debug!("Rate limit exceeded for {}", ip);
            false
        }
    }
    
    fn connection_start(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }
    
    fn connection_end(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Shared state for the HTTP server
struct AppState {
    metrics: RwLock<String>,
    collector: MetricsCollector,
    rate_limiter: RateLimiter,
}

// Use a small thread pool regardless of CPU count
// dc-exporter-rs doesn't need 256 threads on a 256-core server
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    // TRACE-THERMAL-006/009: child bypasses normal NVML, BAR, TUI and config startup.
    if args.thermal_helper {
        return metrics::thermal_worker::run_helper();
    }

    // TUI mode doesn't need logging to stdout
    if !args.tui {
        // Initialize logging
        let log_level = if args.debug { "debug" } else { "info" };
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
            )
            .init();

        info!("DC Exporter v{} starting", VERSION);
    }

    // Only thermal settings are activated here; existing dormant sections keep their behavior.
    let config = if args.config.exists() {
        let config = Config::load(&args.config)
            .with_context(|| format!("failed to load configuration {}", args.config.display()))?;
        if !args.tui {
            info!("Loaded config from {:?}", args.config);
        }
        config
    } else {
        if !args.tui {
            info!("Config file not found, using defaults");
        }
        Config::default()
    };

    let nvml_timeout = Duration::from_secs(args.nvml_timeout);
    let interval = Duration::from_secs(args.interval);
    let thermal_policy = config.thermal.resolve(args.vram_backend, args.thermal_timeout_ms, args.thermal_max_sample_age_ms)?;

    // Check register access status and warn if not available
    if !args.no_register_access && !args.tui {
        let register_status = metrics::RegisterAccessStatus::check();
        if let Some(warning) = register_status.warning_message() {
            warn!("{}", warning);
            eprintln!("\n⚠️  WARNING: {}", warning);
            eprintln!("   This concerns legacy BAR temperatures. NVML/RUSD VRAM telemetry does not require /dev/mem.\n");
        }
    }

    // TUI mode: interactive terminal UI
    if args.tui {
        return tui::run_tui(interval, nvml_timeout, !args.no_register_access, thermal_policy).await;
    }

    // Create metrics collector with safety settings
    let collector = MetricsCollector::with_thermal(nvml_timeout, !args.no_register_access, thermal_policy, interval);

    // One-shot mode: collect and exit
    if args.one_shot {
        return run_one_shot(&collector).await;
    }

    // Continuous mode with HTTP server
    let state = Arc::new(AppState {
        metrics: RwLock::new(String::new()),
        collector,
        rate_limiter: RateLimiter::new(),
    });

    // Start metrics collection loop in background
    let state_clone = Arc::clone(&state);
    
    tokio::spawn(async move {
        metrics_collection_loop(state_clone, interval).await;
    });

    // Start HTTP server
    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    info!("Starting HTTP server on {}", addr);
    
    run_http_server(addr, state).await
}

/// Run in one-shot mode: collect metrics once and print to stdout
async fn run_one_shot(collector: &MetricsCollector) -> Result<()> {
    info!("Running in one-shot mode");
    
    let metrics = collector.collect().await;
    let output_str = PrometheusFormatter::format(&metrics);
    println!("{}", output_str);
    
    Ok(())
}

/// Background task to collect metrics periodically and cache in memory
async fn metrics_collection_loop(state: Arc<AppState>, interval: Duration) {
    loop {
        // Collect metrics (with timeout protection)
        let metrics = state.collector.collect().await;
        let output_str = PrometheusFormatter::format(&metrics);
        
        // Update cached metrics (memory only - no disk writes)
        {
            let mut guard = state.metrics.write().await;
            *guard = output_str;
        }
        
        tokio::time::sleep(interval).await;
    }
}

/// Simple HTTP server for /metrics endpoint with rate limiting
async fn run_http_server(addr: SocketAddr, state: Arc<AppState>) -> Result<()> {
    use http_body_util::Full;
    use hyper::body::Bytes;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind(addr).await?;
    info!("HTTP server listening on {} (rate limit: {} req/s per IP, max {} connections)", 
          addr, MAX_REQUESTS_PER_SECOND, MAX_CONCURRENT_CONNECTIONS);

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let peer_ip = peer_addr.ip();
        let state = Arc::clone(&state);
        
        // Rate limiting check
        if !state.rate_limiter.check(peer_ip).await {
            // Silently drop connection if rate limited
            continue;
        }
        
        // Track connection
        state.rate_limiter.connection_start();
        
        let io = TokioIo::new(stream);
        
        // Clone for cleanup after connection
        let state_cleanup = Arc::clone(&state);

        tokio::task::spawn(async move {
            let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                let state = Arc::clone(&state);
                async move {
                    let path = req.uri().path();
                    
                    match path {
                        "/metrics" | "/" => {
                            let metrics = state.metrics.read().await;
                            let body = if metrics.is_empty() {
                                "# No metrics collected yet\n".to_string()
                            } else {
                                metrics.clone()
                            };
                            
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .header("Content-Type", "text/plain; charset=utf-8")
                                    .body(Full::new(Bytes::from(body)))
                                    .unwrap(),
                            )
                        }
                        "/health" => {
                            Ok(Response::builder()
                                .status(StatusCode::OK)
                                .body(Full::new(Bytes::from("OK\n")))
                                .unwrap())
                        }
                        _ => {
                            Ok(Response::builder()
                                .status(StatusCode::NOT_FOUND)
                                .body(Full::new(Bytes::from("Not Found\n")))
                                .unwrap())
                        }
                    }
                }
            });

            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service)
                .await
            {
                debug!("HTTP connection error: {}", err);
            }
            
            // Connection done
            state_cleanup.rate_limiter.connection_end();
        });
    }
}
