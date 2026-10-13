//! Terminal UI Module
//!
//! Provides an nvtop-like terminal interface for monitoring GPUs.
//! Shows real-time GPU metrics with graphical bars and history.

mod app;
mod ui;

pub use app::run_tui;
