//! Safe NVML Wrapper
//!
//! This module provides timeout-protected wrappers around NVML calls.
//! 
//! CRITICAL SAFETY FEATURE:
//! When GPUs are in VM passthrough or experiencing driver crashes,
//! NVML calls can hang indefinitely (process goes into D-state).
//! 
//! This wrapper:
//! 1. Executes NVML calls in a separate thread
//! 2. Applies a timeout to detect hangs
//! 3. Returns an error instead of blocking forever
//!
//! This ensures dc-exporter NEVER interferes with GPU passthrough operations.
//!
//! NOTE: The persistent NVML worker in metrics/gpu.rs is preferred for
//! production use. This module provides lower-level primitives.

use std::panic;
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use nvml_wrapper::error::NvmlError;
use nvml_wrapper::Nvml;
use thiserror::Error;
use tracing::{debug, info, warn};

// Global NVML availability cache to avoid repeated init attempts
static NVML_AVAILABLE: AtomicBool = AtomicBool::new(false);
static NVML_CHECKED: AtomicBool = AtomicBool::new(false);
static NVML_LAST_CHECK: AtomicU64 = AtomicU64::new(0);
static NVML_TIMEOUT_COUNT: AtomicU64 = AtomicU64::new(0);
const NVML_RECHECK_INTERVAL_SECS: u64 = 300; // Recheck every 5 min (was 60s - too aggressive)
const NVML_MAX_TIMEOUTS: u64 = 3; // After 3 timeouts, stop trying (driver is stuck)

/// Try to create the NVML symlink if missing (common on some systems)
fn ensure_nvml_symlink() {
    use std::path::Path;
    use std::os::unix::fs::symlink;
    
    let lib_paths = [
        ("/lib/x86_64-linux-gnu/libnvidia-ml.so.1", "/lib/x86_64-linux-gnu/libnvidia-ml.so"),
        ("/usr/lib/x86_64-linux-gnu/libnvidia-ml.so.1", "/usr/lib/x86_64-linux-gnu/libnvidia-ml.so"),
        ("/usr/lib64/libnvidia-ml.so.1", "/usr/lib64/libnvidia-ml.so"),
    ];
    
    for (versioned, unversioned) in &lib_paths {
        if Path::new(versioned).exists() && !Path::new(unversioned).exists() {
            if symlink(versioned, unversioned).is_ok() {
                info!("Created NVML symlink: {} -> {}", unversioned, versioned);
            }
        }
    }
}

/// Errors that can occur during safe NVML operations
#[derive(Error, Debug)]
pub enum SafeNvmlError {
    #[error("NVML operation timed out (GPU may be in passthrough or driver crashed)")]
    Timeout,

    #[error("NVML operation panicked")]
    Panic,

    #[error("NVML error: {0}")]
    Nvml(#[from] NvmlError),

    #[error("Failed to spawn worker thread")]
    ThreadSpawn,
    
    #[error("NVML unavailable (library not loaded)")]
    NvmlUnavailable,
}

/// Result type for safe NVML operations
pub type SafeResult<T> = Result<T, SafeNvmlError>;

/// Safe wrapper around NVML that protects against hangs
pub struct SafeNvml {
    timeout: Duration,
}

impl SafeNvml {
    /// Create a new SafeNvml wrapper with the specified timeout
    pub fn new(timeout: Duration) -> Self {
        SafeNvml { timeout }
    }

    /// Execute a closure with NVML access, protected by timeout
    /// 
    /// If the operation hangs (GPU in passthrough, driver crash), this will
    /// return a Timeout error instead of blocking forever.
    pub fn with_timeout<T, F>(&self, f: F) -> SafeResult<T>
    where
        F: FnOnce(&Nvml) -> Result<T, NvmlError> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let timeout = self.timeout;

        // Spawn worker thread to execute NVML operation
        let handle = thread::Builder::new()
            .name("nvml-worker".to_string())
            .spawn(move || {
                // Catch panics to prevent crashing the main thread
                let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                    match Nvml::init() {
                        Ok(nvml) => f(&nvml),
                        Err(e) => {
                            debug!("NVML init failed: {:?}", e);
                            Err(e)
                        }
                    }
                }));
                
                // Send result back (ignore error if receiver dropped due to timeout)
                let _ = tx.send(result);
            });

        let handle = match handle {
            Ok(h) => h,
            Err(e) => {
                warn!("Failed to spawn NVML worker thread: {}", e);
                return Err(SafeNvmlError::ThreadSpawn);
            }
        };

        // Wait for result with timeout
        match rx.recv_timeout(timeout) {
            Ok(Ok(Ok(result))) => {
                // Clean join
                let _ = handle.join();
                Ok(result)
            }
            Ok(Ok(Err(nvml_err))) => {
                let _ = handle.join();
                Err(SafeNvmlError::Nvml(nvml_err))
            }
            Ok(Err(_panic)) => {
                warn!("NVML operation panicked");
                // Don't join panicked thread
                Err(SafeNvmlError::Panic)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                warn!(
                    "NVML operation timed out after {:?} - GPU may be in passthrough or driver crashed",
                    timeout
                );
                // Don't join timed-out thread - it may be in D-state
                // The thread will be orphaned but that's better than blocking
                Err(SafeNvmlError::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Worker thread died unexpectedly
                warn!("NVML worker thread died unexpectedly");
                Err(SafeNvmlError::Panic)
            }
        }
    }

    /// Check if NVML is available (with timeout protection and caching)
    ///
    /// SAFETY: After NVML_MAX_TIMEOUTS consecutive timeouts, we permanently
    /// mark NVML as unavailable. This prevents orphaned threads accumulating
    /// when the NVIDIA driver is stuck (e.g., broken GPU, driver crash).
    /// Each timeout leaves an unkillable D-state thread.
    pub fn check_available(&self) -> bool {
        // If we've hit too many timeouts, the driver is stuck -- stop trying
        let timeout_count = NVML_TIMEOUT_COUNT.load(Ordering::Relaxed);
        if timeout_count >= NVML_MAX_TIMEOUTS {
            // Already gave up -- don't spawn more threads
            return false;
        }

        // Check if we've recently verified NVML availability
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let last_check = NVML_LAST_CHECK.load(Ordering::Relaxed);
        
        // Use cached value if checked recently
        if NVML_CHECKED.load(Ordering::Relaxed) && (now - last_check) < NVML_RECHECK_INTERVAL_SECS {
            return NVML_AVAILABLE.load(Ordering::Relaxed);
        }
        
        debug!("Checking NVML availability (timeout_count: {})", timeout_count);
        
        // Single attempt (no retry -- each timeout orphans a thread)
        let available = match self.with_timeout(|_nvml| Ok(())) {
            Ok(_) => {
                debug!("NVML is available");
                NVML_TIMEOUT_COUNT.store(0, Ordering::Relaxed); // Reset on success
                true
            }
            Err(SafeNvmlError::Timeout) => {
                let new_count = NVML_TIMEOUT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if new_count >= NVML_MAX_TIMEOUTS {
                    warn!(
                        "NVML timed out {} times - disabling GPU collection permanently. \
                         NVIDIA driver appears stuck. Restart dc-exporter to retry.",
                        new_count
                    );
                } else {
                    warn!(
                        "NVML timed out ({}/{}) - GPU driver may be stuck",
                        new_count, NVML_MAX_TIMEOUTS
                    );
                }
                false
            }
            Err(e) => {
                // Non-timeout error (library not found, etc) -- safe, no orphan thread
                if !NVML_CHECKED.load(Ordering::Relaxed) {
                    // First check: try symlink fix
                    ensure_nvml_symlink();
                    match self.with_timeout(|_nvml| Ok(())) {
                        Ok(_) => {
                            info!("NVML available after symlink fix");
                            true
                        }
                        Err(e2) => {
                            warn!("NVML not available: {}", e2);
                            false
                        }
                    }
                } else {
                    debug!("NVML not available: {}", e);
                    false
                }
            }
        };
        
        // Cache the result
        NVML_AVAILABLE.store(available, Ordering::Relaxed);
        NVML_CHECKED.store(true, Ordering::Relaxed);
        NVML_LAST_CHECK.store(now, Ordering::Relaxed);
        
        available
    }

    /// Get device count (with timeout protection)
    pub fn device_count(&self) -> SafeResult<u32> {
        self.with_timeout(|nvml| nvml.device_count())
    }

    /// Get driver version (with timeout protection)
    pub fn driver_version(&self) -> SafeResult<String> {
        self.with_timeout(|nvml| nvml.sys_driver_version())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeout_behavior() {
        let safe = SafeNvml::new(Duration::from_millis(100));
        
        // This should not hang even if NVML is unavailable
        let result = safe.check_available();
        // Just verify it returns (doesn't hang)
        println!("NVML available: {}", result);
    }
}
