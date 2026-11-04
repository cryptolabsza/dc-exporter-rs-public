//! Resolve one NVML provider; native initialization and symbol errors never retry.
// TRACEWEAVER: req=REQ-NVML-001; trace=TRACE-NVML-001; ver=VER-NVML-001
use nvml_wrapper::{error::NvmlError, Nvml};
use std::ffi::OsStr;

#[derive(Debug)]
pub(super) struct NvmlLoadError {
    pub error: NvmlError,
    preferred_error: Option<NvmlError>,
}
impl From<NvmlError> for NvmlLoadError {
    fn from(error: NvmlError) -> Self { Self { error, preferred_error: None } }
}
impl std::fmt::Display for NvmlLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(preferred) = &self.preferred_error {
            write!(f, "libnvidia-ml.so.1: {preferred}; libnvidia-ml.so: {}", self.error)
        } else { std::fmt::Display::fmt(&self.error, f) }
    }
}
impl std::error::Error for NvmlLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> { Some(&self.error) }
}

/// Return the exact candidate passed to the wrapper for RTLD_NOLOAD ownership.
pub(super) fn load() -> Result<(Nvml, &'static OsStr), NvmlLoadError> {
    #[cfg(target_os = "linux")]
    {
        let preferred = OsStr::new("libnvidia-ml.so.1");
        match Nvml::builder().lib_path(preferred).init() {
            Ok(nvml) => return Ok((nvml, preferred)),
            Err(preferred_error @ NvmlError::LibloadingError(_)) => {
                let legacy = OsStr::new("libnvidia-ml.so");
                return match Nvml::builder().lib_path(legacy).init() {
                    Ok(nvml) => {
                        tracing::warn!("NVML libnvidia-ml.so.1: {}; selecting legacy libnvidia-ml.so", preferred_error);
                        Ok((nvml, legacy))
                    }
                    Err(error) => Err(NvmlLoadError { error, preferred_error: Some(preferred_error) }),
                };
            }
            Err(error) => return Err(error.into()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let path = OsStr::new("libnvidia-ml.so");
        Nvml::builder().lib_path(path).init().map(|nvml| (nvml, path)).map_err(Into::into)
    }
}
