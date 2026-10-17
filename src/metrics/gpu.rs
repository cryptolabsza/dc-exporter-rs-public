//! GPU Metrics Collection via NVML
//!
//! Collects standard GPU metrics using NVIDIA's NVML library.
//! Uses a PERSISTENT worker thread to avoid file descriptor leaks
//! from repeatedly spawning threads and initializing NVML.
//!
//! SAFETY: Includes watchdog to detect and recover from stuck workers
//! (e.g., when GPUs are in VM passthrough or driver crash).

use std::sync::mpsc::{self, Sender, Receiver};
use std::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use std::sync::{Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tracing::{error, info, warn};

use nvml_wrapper::Nvml;
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};

use crate::state::{GpuArchitecture, GpuState, ThrottleReasons};
use super::{GpuMetrics, GpuProcess, MetricSource, ProcessType};

/// Fixed query vocabulary; no NVML/error payload becomes a metric label.
#[derive(Clone, Copy, Debug)]
pub enum NvmlQuery {
    DeviceByIndex, Name, Uuid, PciInfo, Temperature, Memory, EccMode,
    EccCorrected, EccUncorrected, PowerUsage, PowerLimit, PowerConstraints,
    SmClock, MemoryClock, Utilization, EncoderUtilization, DecoderUtilization,
    FanSpeed, ThrottleReasons, PcieTx, PcieRx, PcieLinkSpeed, NvlinkState,
    ComputeProcesses, GraphicsProcesses, ComputeCapability, RecoveryAction,
}
impl NvmlQuery {
    fn as_str(self) -> &'static str {
        match self {
            Self::DeviceByIndex=>"device_by_index",Self::Name=>"name",Self::Uuid=>"uuid",Self::PciInfo=>"pci_info",
            Self::Temperature=>"temperature",Self::Memory=>"memory",Self::EccMode=>"ecc_mode",Self::EccCorrected=>"ecc_corrected",Self::EccUncorrected=>"ecc_uncorrected",
            Self::PowerUsage=>"power_usage",Self::PowerLimit=>"power_limit",Self::PowerConstraints=>"power_constraints",Self::SmClock=>"sm_clock",Self::MemoryClock=>"memory_clock",
            Self::Utilization=>"utilization",Self::EncoderUtilization=>"encoder_utilization",Self::DecoderUtilization=>"decoder_utilization",Self::FanSpeed=>"fan_speed",
            Self::ThrottleReasons=>"throttle_reasons",Self::PcieTx=>"pcie_tx",Self::PcieRx=>"pcie_rx",Self::PcieLinkSpeed=>"pcie_link_speed",Self::NvlinkState=>"nvlink_state",
            Self::ComputeProcesses=>"compute_processes",Self::GraphicsProcesses=>"graphics_processes",Self::ComputeCapability=>"compute_capability",Self::RecoveryAction=>"recovery_action",
        }
    }
    fn essential(self) -> bool {matches!(self,Self::Name|Self::Temperature|Self::Memory)}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NvmlQueryError {pub query:&'static str,pub error:&'static str,pub code:Option<u32>}
#[derive(Clone, Debug)]
pub struct UnmatchedNvmlError {pub index:u32,pub uuid:String,pub pci_bus_id:String,pub name:String,pub driver_version:String,pub error:NvmlQueryError,pub recovery:Option<RecoveryObservation>}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryStatus {Valid,Unsupported,Unavailable,Invalid}
impl RecoveryStatus {
    pub fn as_str(self)->&'static str {match self {Self::Valid=>"valid",Self::Unsupported=>"unsupported",Self::Unavailable=>"unavailable",Self::Invalid=>"invalid"}}
}
#[derive(Clone, Debug)]
pub struct RecoveryObservation {pub action:Option<u32>,pub status:RecoveryStatus,pub error:Option<NvmlQueryError>}
impl RecoveryObservation {
    fn failure(status:RecoveryStatus,error:NvmlQueryError)->Self {Self{action:None,status,error:Some(error)}}
}
fn typed_query_error(query:NvmlQuery,error:&NvmlError)->NvmlQueryError {
    use NvmlError::*;
    let (class,code)=match error {
        Uninitialized=>("uninitialized",Some(1)),InvalidArg=>("invalid_argument",Some(2)),NotSupported=>("not_supported",Some(3)),NoPermission=>("permission_denied",Some(4)),
        NotFound=>("not_found",Some(6)),InsufficientSize(_)=>("insufficient_size",Some(7)),InsufficientPower=>("insufficient_power",Some(8)),DriverNotLoaded=>("driver_not_loaded",Some(9)),
        Timeout=>("timeout",Some(10)),IrqIssue=>("irq_issue",Some(11)),LibraryNotFound=>("library_not_found",Some(12)),FunctionNotFound=>("function_not_found",Some(13)),
        CorruptedInfoROM=>("corrupted_inforom",Some(14)),GpuLost=>("gpu_lost",Some(15)),ResetRequired=>("reset_required",Some(16)),OperatingSystem=>("operating_system",Some(17)),
        LibRmVersionMismatch=>("driver_library_mismatch",Some(18)),InUse=>("in_use",Some(19)),InsufficientMemory=>("insufficient_memory",Some(20)),NoData=>("no_data",Some(21)),
        VgpuEccNotSupported=>("vgpu_ecc_not_supported",Some(22)),Unknown=>("unknown",Some(999)),
        FailedToLoadSymbol(_)=>("symbol_unavailable",None),UnexpectedVariant(_)|Utf8Error(_)|NulError(_)|IncorrectBits(_)|StringTooLong{..}|GetPciInfoFailed|PciInfoToCFailed=>("conversion_error",None),
        _=>("other",None),
    };
    NvmlQueryError{query:query.as_str(),error:class,code}
}
fn native_recovery_error(code:u32)->NvmlQueryError {
    let error=match code {
        1=>NvmlError::Uninitialized,2=>NvmlError::InvalidArg,3=>NvmlError::NotSupported,4=>NvmlError::NoPermission,6=>NvmlError::NotFound,
        7=>NvmlError::InsufficientSize(None),8=>NvmlError::InsufficientPower,9=>NvmlError::DriverNotLoaded,10=>NvmlError::Timeout,11=>NvmlError::IrqIssue,
        12=>NvmlError::LibraryNotFound,13=>NvmlError::FunctionNotFound,14=>NvmlError::CorruptedInfoROM,15=>NvmlError::GpuLost,16=>NvmlError::ResetRequired,
        17=>NvmlError::OperatingSystem,18=>NvmlError::LibRmVersionMismatch,19=>NvmlError::InUse,20=>NvmlError::InsufficientMemory,21=>NvmlError::NoData,22=>NvmlError::VgpuEccNotSupported,
        999=>NvmlError::Unknown,_=>NvmlError::UnexpectedVariant(code),
    };
    typed_query_error(NvmlQuery::RecoveryAction,&error)
}
fn retain_query_error(gpu:&mut GpuMetrics,error:NvmlQueryError) {
    if !gpu.nvml_errors.contains(&error) {gpu.nvml_errors.push(error);}
}
pub(super) fn health_priority(state:GpuState)->u8 {match state {GpuState::HardwareFailure=>3,GpuState::Error=>2,GpuState::Unavailable=>1,_=>0}}
fn promote_health(gpu:&mut GpuMetrics,state:GpuState) {if health_priority(state)>health_priority(gpu.state) {gpu.state=state;}}
pub(super) fn record_nvml_query_error(gpu:&mut GpuMetrics,query:NvmlQuery,error:&NvmlError) {
    let diagnostic=typed_query_error(query,error);
    let unsupported=matches!(error,NvmlError::NotSupported|NvmlError::FunctionNotFound|NvmlError::FailedToLoadSymbol(_));
    if query.essential() {gpu.nvml_essential_query_failed=true;}
    if query.essential() || matches!(error,NvmlError::GpuLost|NvmlError::ResetRequired) {promote_health(gpu,GpuState::Error);}
    else if !unsupported {promote_health(gpu,GpuState::Unavailable);}
    retain_query_error(gpu,diagnostic);
}

// Native LP64 ABI from locked nvml-wrapper-sys0.8.0. Preserve ULONG before
// nvml-wrapper0.10's SampleValue conversion can narrow it to u32.
#[repr(C)]
union RawNvmlValue {bits:u64,double:f64,unsigned_long:libc::c_ulong,signed_long_long:i64,signed_int:libc::c_int,unsigned_int:u32}
#[repr(C)]
struct RawFieldValue {field_id:u32,scope_id:u32,timestamp:i64,latency_us:i64,value_type:u32,nvml_return:u32,value:RawNvmlValue}
type RawFieldFunction=unsafe extern "C" fn(*mut libc::c_void,libc::c_int,*mut RawFieldValue)->libc::c_uint;
struct RawNvml {library:*mut libc::c_void,field:RawFieldFunction}
impl Drop for RawNvml {fn drop(&mut self) {unsafe {libc::dlclose(self.library);}}}
enum RawAccess {Available(RawNvml),Unsupported,Unavailable}
/// One wrapper session and one extra reference to its exact loaded library.
/// Raw access never initializes/shuts down NVML or accepts a foreign Device.
pub(super) struct NvmlContext {raw:RawAccess,nvml:Nvml}
impl NvmlContext {
    fn new()->Result<Self,NvmlError> {Self::open(std::ffi::OsStr::new("libnvidia-ml.so"))}
    fn open(path:&std::ffi::OsStr)->Result<Self,NvmlError> {
        use std::os::unix::ffi::OsStrExt;
        let nvml=Nvml::builder().lib_path(path).init()?;
        let target=std::ffi::CString::new(path.as_bytes())?;
        // RTLD_NOLOAD forbids substituting another provider after wrapper init.
        let library=unsafe {libc::dlopen(target.as_ptr(),libc::RTLD_NOLOAD|libc::RTLD_NOW|libc::RTLD_LOCAL)};
        let raw=if library.is_null() {RawAccess::Unavailable} else {
            let symbol=unsafe {libc::dlsym(library,b"nvmlDeviceGetFieldValues\0".as_ptr().cast())};
            if symbol.is_null() {unsafe {libc::dlclose(library);}RawAccess::Unsupported}
            else {RawAccess::Available(RawNvml{library,field:unsafe {std::mem::transmute::<*mut libc::c_void,RawFieldFunction>(symbol)}})}
        };
        Ok(Self{raw,nvml})
    }
    #[cfg(test)]
    pub(super) fn for_test(path:&std::ffi::OsStr)->Result<Self,NvmlError> {Self::open(path)}
    fn recovery_observation(&self,device:&nvml_wrapper::Device<'_>)->RecoveryObservation {
        let invalid=||RecoveryObservation::failure(RecoveryStatus::Invalid,NvmlQueryError{query:"recovery_action",error:"invalid_data",code:None});
        if !std::ptr::eq(device.nvml(),&self.nvml) {
            return RecoveryObservation::failure(RecoveryStatus::Unavailable,NvmlQueryError{query:"recovery_action",error:"invalid_data",code:None});
        }
        let raw=match &self.raw {
            RawAccess::Available(raw)=>raw,
            RawAccess::Unsupported=>return RecoveryObservation::failure(RecoveryStatus::Unsupported,NvmlQueryError{query:"recovery_action",error:"symbol_unavailable",code:None}),
            RawAccess::Unavailable=>return RecoveryObservation::failure(RecoveryStatus::Unavailable,NvmlQueryError{query:"recovery_action",error:"library_not_found",code:None}),
        };
        let mut field=RawFieldValue{field_id:230,scope_id:0,timestamp:0,latency_us:0,value_type:u32::MAX,nvml_return:u32::MAX,value:RawNvmlValue{bits:0}};
        let status=unsafe {(raw.field)(device.handle().cast(),1,&mut field)};
        // An untouched output is not evidence that the driver returned native999.
        if status==0 && field.nvml_return==u32::MAX {return invalid();}
        let status=if status!=0 {status} else {field.nvml_return};
        if status!=0 {
            return RecoveryObservation::failure(if status==3 || status==13 {RecoveryStatus::Unsupported}else{RecoveryStatus::Unavailable},native_recovery_error(status));
        }
        if field.field_id!=230 || field.scope_id!=0 || field.timestamp<0 || field.latency_us<0 {return invalid();}
        match decode_native_recovery_value(field.value_type,unsafe {field.value.bits}) {
            Some(action)=>RecoveryObservation{action:Some(action),status:RecoveryStatus::Valid,error:None},None=>invalid(),
        }
    }
}
fn decode_native_recovery_value(tag:u32,bits:u64)->Option<u32> {
    match tag {
        0=>{let v=f64::from_bits(bits);if v.is_finite() && v>=0.0 && v<=u32::MAX as f64 && v.fract()==0.0 {Some(v as u32)}else{None}},
        1=>Some(bits as u32),
        2=>u32::try_from(bits as libc::c_ulong).ok(),3=>u32::try_from(bits).ok(),
        4=>u32::try_from(bits as i64).ok(),5=>u32::try_from(bits as u32 as i32).ok(),_=>None,
    }
}

/// Maximum consecutive timeouts before declaring worker stuck
const MAX_CONSECUTIVE_TIMEOUTS: u32 = 3;

/// Global NVML worker manager (supports restart)
static NVML_MANAGER: RwLock<Option<NvmlWorkerManager>> = RwLock::new(None);

/// Result of NVML collection - includes status info
#[derive(Debug)]
pub struct NvmlCollectionResult {
    pub gpus: Vec<GpuMetrics>,
    pub nvml_initialized: bool,  // false = driver error, true = NVML worked
    pub unmatched_errors: Vec<UnmatchedNvmlError>,
}

/// Request to the NVML worker
enum NvmlRequest {
    CollectAll(Sender<NvmlCollectionResult>),
    Ping(Sender<()>),  // Health check
    Shutdown,
}

/// Manages the NVML worker with watchdog capabilities
struct NvmlWorkerManager {
    worker: NvmlWorker,
    consecutive_timeouts: AtomicU32,
    last_success_epoch: AtomicU64,
    generation: u32,  // Incremented on each restart
}

impl NvmlWorkerManager {
    fn new(generation: u32) -> Self {
        NvmlWorkerManager {
            worker: NvmlWorker::new(),
            consecutive_timeouts: AtomicU32::new(0),
            last_success_epoch: AtomicU64::new(current_epoch_secs()),
            generation,
        }
    }
    
    fn is_stuck(&self) -> bool {
        self.consecutive_timeouts.load(Ordering::Relaxed) >= MAX_CONSECUTIVE_TIMEOUTS
    }
    
    fn record_success(&self) {
        self.consecutive_timeouts.store(0, Ordering::Relaxed);
        self.last_success_epoch.store(current_epoch_secs(), Ordering::Relaxed);
    }
    
    fn record_timeout(&self) {
        let count = self.consecutive_timeouts.fetch_add(1, Ordering::Relaxed) + 1;
        warn!("NVML worker timeout #{} (max {})", count, MAX_CONSECUTIVE_TIMEOUTS);
    }
}

/// Get current time as epoch seconds
fn current_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Persistent NVML worker that keeps a single NVML session alive
struct NvmlWorker {
    tx: Mutex<Sender<NvmlRequest>>,
    handle: Option<JoinHandle<()>>,
}

impl NvmlWorker {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel::<NvmlRequest>();
        
        let handle = thread::Builder::new()
            .name("nvml-worker".to_string())
            .spawn(move || {
                Self::worker_loop(rx);
            })
            .expect("Failed to spawn NVML worker thread");
        
        NvmlWorker {
            tx: Mutex::new(tx),
            handle: Some(handle),
        }
    }
    
    fn worker_loop(rx: Receiver<NvmlRequest>) {
        info!("NVML worker thread started");
        
        // Keep NVML initialized for the lifetime of the worker
        let (nvml, nvml_initialized) = match NvmlContext::new() {
            Ok(n) => {
                info!("NVML initialized successfully");
                (Some(n), true)
            }
            Err(e) => {
                warn!("Failed to initialize NVML: {:?}", e);
                (None, false)
            }
        };
        
        // Track if we ever successfully init NVML
        let mut ever_initialized = nvml_initialized;
        
        while let Ok(request) = rx.recv() {
            match request {
                NvmlRequest::CollectAll(response_tx) => {
                    let result = if let Some(ref nvml) = nvml {
                        collect_metrics_from_nvml(nvml)
                    } else {
                        // Try to init again in case driver was loaded
                        match NvmlContext::new() {
                            Ok(nvml) => {
                                ever_initialized = true;
                                collect_metrics_from_nvml(&nvml)
                            }
                            Err(_) => NvmlCollectionResult {
                                gpus: Vec::new(),
                                unmatched_errors: Vec::new(),
                                nvml_initialized: ever_initialized,
                            },
                        }
                    };
                    let _ = response_tx.send(result);
                }
                NvmlRequest::Ping(response_tx) => {
                    let _ = response_tx.send(());
                }
                NvmlRequest::Shutdown => {
                    info!("NVML worker shutting down");
                    break;
                }
            }
        }
        
        // NVML is dropped here
        info!("NVML worker thread exiting");
    }
    
    fn collect_all(&self, timeout: Duration) -> Option<NvmlCollectionResult> {
        let (response_tx, response_rx) = mpsc::channel();
        
        // Send request to worker
        {
            let tx = self.tx.lock().unwrap();
            if tx.send(NvmlRequest::CollectAll(response_tx)).is_err() {
                warn!("NVML worker channel closed");
                return None;
            }
        }
        
        // Wait for response with timeout
        match response_rx.recv_timeout(timeout) {
            Ok(result) => Some(result),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                warn!("NVML worker disconnected");
                None
            }
        }
    }
    
    /// Check if worker is responsive
    fn ping(&self, timeout: Duration) -> bool {
        let (response_tx, response_rx) = mpsc::channel();
        
        {
            let tx = self.tx.lock().unwrap();
            if tx.send(NvmlRequest::Ping(response_tx)).is_err() {
                return false;
            }
        }
        
        response_rx.recv_timeout(timeout).is_ok()
    }
    
    /// Request shutdown (non-blocking)
    fn shutdown(&self) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(NvmlRequest::Shutdown);
        }
    }
}

/// Get or create the NVML worker manager, restarting if stuck
fn get_or_restart_worker() -> Option<std::sync::RwLockReadGuard<'static, Option<NvmlWorkerManager>>> {
    // Fast path: check if manager exists and is healthy
    {
        let guard = NVML_MANAGER.read().unwrap();
        if let Some(ref manager) = *guard {
            if !manager.is_stuck() {
                return Some(guard);
            }
        }
    }
    
    // Slow path: need to create or restart
    let mut guard = NVML_MANAGER.write().unwrap();
    
    let new_generation = if let Some(ref old_manager) = *guard {
        if old_manager.is_stuck() {
            warn!("NVML worker stuck after {} consecutive timeouts - restarting (gen {})",
                  MAX_CONSECUTIVE_TIMEOUTS, old_manager.generation);
            
            // Try to shutdown old worker gracefully
            old_manager.worker.shutdown();
            
            old_manager.generation + 1
        } else {
            // Another thread fixed it, we're good
            drop(guard);
            return Some(NVML_MANAGER.read().unwrap());
        }
    } else {
        info!("Initializing NVML worker manager");
        0
    };
    
    // Create new manager
    *guard = Some(NvmlWorkerManager::new(new_generation));
    info!("NVML worker manager started (generation {})", new_generation);
    
    drop(guard);
    Some(NVML_MANAGER.read().unwrap())
}

/// Collect metrics for all available GPUs via NVML
pub struct NvmlCollector {
    timeout: Duration,
}

impl NvmlCollector {
    pub fn new(timeout: Duration) -> Self {
        // Ensure worker is initialized
        let _ = get_or_restart_worker();
        NvmlCollector { timeout }
    }

    /// Collect metrics for ALL GPUs using the persistent NVML worker
    /// Includes watchdog: if worker is stuck, it will be restarted
    /// Returns (gpus, nvml_initialized) - nvml_initialized is false if driver error
    pub fn collect_all(&self) -> NvmlCollectionResult {
        let guard = match get_or_restart_worker() {
            Some(g) => g,
            None => {
                error!("Failed to get NVML worker manager");
                return NvmlCollectionResult {
                    gpus: Vec::new(),
                    unmatched_errors: Vec::new(),
                    nvml_initialized: false,
                };
            }
        };
        
        let manager = match guard.as_ref() {
            Some(m) => m,
            None => {
                error!("NVML worker manager is None");
                return NvmlCollectionResult {
                    gpus: Vec::new(),
                    unmatched_errors: Vec::new(),
                    nvml_initialized: false,
                };
            }
        };
        
        // Use timeout * 2 for actual collection
        let timeout = self.timeout * 2;
        
        match manager.worker.collect_all(timeout) {
            Some(result) => {
                manager.record_success();
                result
            }
            None => {
                manager.record_timeout();
                
                // If now stuck, the next call will restart the worker
                if manager.is_stuck() {
                    warn!("NVML worker declared stuck - will restart on next collection");
                }
                
                NvmlCollectionResult {
                    gpus: Vec::new(),
                    unmatched_errors: Vec::new(),
                    nvml_initialized: false,
                }
            }
        }
    }
}

/// Collect current device evidence without allowing later success to erase faults.
pub(super) fn collect_metrics_from_nvml(context:&NvmlContext)->NvmlCollectionResult {
    let nvml=&context.nvml;
    let count=match nvml.device_count() {Ok(n)=>n,Err(e)=>{warn!("NVML enumeration failed: {}",e);return NvmlCollectionResult{gpus:Vec::new(),unmatched_errors:Vec::new(),nvml_initialized:true};}};
    let driver=nvml.sys_driver_version().unwrap_or_else(|_|"unknown".into());
    let mut metrics=Vec::with_capacity(count as usize);let mut unmatched_errors=Vec::new();
    for i in 0..count {
        let device=match nvml.device_by_index(i) {Ok(d)=>d,Err(error)=>{
            unmatched_errors.push(UnmatchedNvmlError{index:i,uuid:String::new(),pci_bus_id:String::new(),name:String::new(),driver_version:driver.clone(),recovery:None,error:typed_query_error(NvmlQuery::DeviceByIndex,&error)});continue;
        }};
        let mut gpu=GpuMetrics{index:i,source:MetricSource::Host,driver_version:driver.clone(),..Default::default()};
        macro_rules! probe {($query:ident,$call:expr)=>{match $call {Ok(value)=>Some(value),Err(error)=>{record_nvml_query_error(&mut gpu,NvmlQuery::$query,&error);None}}};}
        // Identity probes are independent, including after an essential name failure.
        if let Some(name)=probe!(Name,device.name()) {gpu.architecture=GpuArchitecture::from_name(&name);gpu.name=name;}
        if let Some(uuid)=probe!(Uuid,device.uuid()) {gpu.uuid=uuid;}
        if let Some(pci)=probe!(PciInfo,device.pci_info()) {
            // NVML supplies a full bus ID; preserve its function, never fabricate .0.
            if let Some(id)=super::dmesg::canonical_pci_token(&pci.bus_id) {gpu.pci_bus_id=id;}
            else {record_nvml_query_error(&mut gpu,NvmlQuery::PciInfo,&NvmlError::GetPciInfoFailed);}
        }
        if let Some(temp)=probe!(Temperature,device.temperature(TemperatureSensor::Gpu)) {gpu.gpu_temp=Some(temp);}
        if let Some(mem)=probe!(Memory,device.memory_info()) {gpu.fb_used=Some(mem.used/(1024*1024));gpu.fb_free=Some(mem.free/(1024*1024));gpu.fb_total=Some(mem.total/(1024*1024));}
        let recovery=context.recovery_observation(&device);
        if let Some(error)=recovery.error.clone() {
            if error.code==Some(15) || error.code==Some(16) {promote_health(&mut gpu,GpuState::Error);}
            else if recovery.status!=RecoveryStatus::Unsupported {promote_health(&mut gpu,GpuState::Unavailable);}
            retain_query_error(&mut gpu,error);
        }
        if recovery.action.is_some_and(|action|action>0) {promote_health(&mut gpu,GpuState::Error);}
        gpu.recovery=Some(recovery);
        if let Some(mode)=probe!(EccMode,device.is_ecc_enabled()) {gpu.ecc_enabled=mode.currently_enabled;}
        if let Some(errors)=probe!(EccCorrected,device.total_ecc_errors(nvml_wrapper::enum_wrappers::device::MemoryError::Corrected,nvml_wrapper::enum_wrappers::device::EccCounter::Volatile)) {gpu.ecc_sbe=Some(errors);}
        if let Some(errors)=probe!(EccUncorrected,device.total_ecc_errors(nvml_wrapper::enum_wrappers::device::MemoryError::Uncorrected,nvml_wrapper::enum_wrappers::device::EccCounter::Volatile)) {gpu.ecc_dbe=Some(errors);if errors>0 && gpu.state==GpuState::Ok {gpu.state=GpuState::Warning;}}
        if let Some(mw)=probe!(PowerUsage,device.power_usage()) {gpu.power_usage=Some(mw as f64/1000.0);}
        if let Some(mw)=probe!(PowerLimit,device.enforced_power_limit()) {gpu.power_limit=Some(mw as f64/1000.0);}
        if let Some(limits)=probe!(PowerConstraints,device.power_management_limit_constraints()) {gpu.power_max=Some(limits.max_limit as f64/1000.0);}
        if let Some(value)=probe!(SmClock,device.clock_info(Clock::SM)) {gpu.sm_clock=Some(value);}
        if let Some(value)=probe!(MemoryClock,device.clock_info(Clock::Memory)) {gpu.mem_clock=Some(value);}
        if let Some(value)=probe!(Utilization,device.utilization_rates()) {gpu.gpu_util=Some(value.gpu);gpu.mem_util=Some(value.memory);}
        if let Some(value)=probe!(EncoderUtilization,device.encoder_utilization()) {gpu.encoder_util=Some(value.utilization);}
        if let Some(value)=probe!(DecoderUtilization,device.decoder_utilization()) {gpu.decoder_util=Some(value.utilization);}
        if matches!(gpu.architecture,GpuArchitecture::Datacenter) {gpu.fan_speed=Some(0);gpu.vram_temp=Some(0);gpu.hotspot_temp=Some(0);}
        else if let Some(value)=probe!(FanSpeed,device.fan_speed(0)) {gpu.fan_speed=Some(value);}
        if let Some(value)=probe!(ThrottleReasons,device.current_throttle_reasons()) {gpu.throttle_reasons=ThrottleReasons::from_bitmask(value.bits());}
        if let Some(value)=probe!(PcieTx,device.pcie_throughput(nvml_wrapper::enum_wrappers::device::PcieUtilCounter::Send)) {gpu.pcie_tx_throughput=Some(value as u64);}
        if let Some(value)=probe!(PcieRx,device.pcie_throughput(nvml_wrapper::enum_wrappers::device::PcieUtilCounter::Receive)) {gpu.pcie_rx_throughput=Some(value as u64);}
        if let Some(speed)=probe!(PcieLinkSpeed,device.pcie_link_speed()) {let (generation,width)=estimate_pcie_gen_width(speed);gpu.pcie_gen=Some(generation);gpu.pcie_width=Some(width);}
        collect_nvlink_metrics(&device,&mut gpu);
        collect_process_info(&device,&mut gpu);
        if matches!(gpu.architecture,GpuArchitecture::Unknown) {
            if let Some(cap)=probe!(ComputeCapability,device.cuda_compute_capability()) {gpu.architecture=GpuArchitecture::from_compute_capability(cap.major,cap.minor);}
        }
        metrics.push(gpu);
    }
    NvmlCollectionResult{gpus:metrics,unmatched_errors,nvml_initialized:true}
}

/// Estimate PCIe generation and width from speed (MB/s)
fn estimate_pcie_gen_width(speed_mbps: u32) -> (u32, u32) {
    // PCIe speeds per lane: Gen3=~1GB/s, Gen4=~2GB/s, Gen5=~4GB/s
    // Common configs: x1, x4, x8, x16
    match speed_mbps {
        // Gen5
        s if s >= 60000 => (5, 16),
        s if s >= 30000 => (5, 8),
        s if s >= 15000 => (5, 4),
        // Gen4
        s if s >= 28000 => (4, 16),
        s if s >= 14000 => (4, 8),
        s if s >= 7000 => (4, 4),
        // Gen3
        s if s >= 14000 => (3, 16),
        s if s >= 7000 => (3, 8),
        s if s >= 3500 => (3, 4),
        // Gen2 or lower
        s if s >= 4000 => (2, 16),
        _ => (3, 16), // Default assumption
    }
}

/// Collect NVLink metrics for a device
fn collect_nvlink_metrics(device: &nvml_wrapper::Device, gpu: &mut GpuMetrics) {
    // NVLink is typically available on A100, H100, V100, etc.
    // Try to get NVLink info - typically up to 18 links on newer GPUs
    let max_links = 18u32;
    let mut active_links = 0u32;
    
    for link in 0..max_links {
        // Use the link_wrapper_for API to get NvLink object
        let nvlink = device.link_wrapper_for(link);
        
        // NVML InvalidArg can mean either a bad device or an out-of-range link.
        // Exempt only context-proven absence: A100 has12 links, L40/L4 has none.
        // Unexpected in-range InvalidArg and undocumented NotFound stay unknown.
        let known_absent=gpu.name.split_whitespace().any(|part|matches!(part,"L4"|"L40"|"L40S"))
            || (gpu.name.contains("A100") && link>=12);
        match nvlink.is_active() {
            Ok(active)=>{if active {active_links+=1;}},
            Err(NvmlError::InvalidArg) if known_absent=>retain_query_error(gpu,NvmlQueryError{query:"nvlink_state",error:"unsupported_slot",code:Some(2)}),
            Err(error)=>record_nvml_query_error(gpu,NvmlQuery::NvlinkState,&error),
        }
    }
    
    gpu.nvlink_active = active_links;
    gpu.nvlink_total = max_links;
    
    // Note: throughput and error counters would require additional NVML calls
    // that may not be available in nvml-wrapper 0.10
    // We just report active link count for now
}

/// Extract memory in MiB from UsedGpuMemory enum
fn get_used_memory_mib(mem: &nvml_wrapper::enums::device::UsedGpuMemory) -> u64 {
    match mem {
        nvml_wrapper::enums::device::UsedGpuMemory::Used(bytes) => bytes / (1024 * 1024),
        nvml_wrapper::enums::device::UsedGpuMemory::Unavailable => 0,
    }
}

/// Collect process information for a device
fn collect_process_info(device: &nvml_wrapper::Device, gpu: &mut GpuMetrics) {
    // Get compute processes
    let compute=match device.running_compute_processes() {Ok(v)=>Some(v),Err(error)=>{record_nvml_query_error(gpu,NvmlQuery::ComputeProcesses,&error);None}};
    if let Some(procs) = compute {
        gpu.compute_processes = procs.len() as u32;
        
        for proc in procs.iter().take(10) {  // Limit to 10 processes
            let name = get_process_name(proc.pid);
            let used_memory = get_used_memory_mib(&proc.used_gpu_memory);
            gpu.process_list.push(GpuProcess {
                pid: proc.pid,
                name,
                used_memory,
                process_type: ProcessType::Compute,
            });
        }
    }
    
    // Get graphics processes
    let graphics=match device.running_graphics_processes() {Ok(v)=>Some(v),Err(error)=>{record_nvml_query_error(gpu,NvmlQuery::GraphicsProcesses,&error);None}};
    if let Some(procs) = graphics {
        gpu.graphics_processes = procs.len() as u32;
        
        for proc in procs.iter().take(10) {
            let used_memory = get_used_memory_mib(&proc.used_gpu_memory);
            // Check if we already have this PID (mixed compute/graphics)
            let existing = gpu.process_list.iter_mut().find(|p| p.pid == proc.pid);
            if let Some(existing) = existing {
                existing.process_type = ProcessType::Mixed;
                // Add memory usage
                existing.used_memory += used_memory;
            } else {
                let name = get_process_name(proc.pid);
                gpu.process_list.push(GpuProcess {
                    pid: proc.pid,
                    name,
                    used_memory,
                    process_type: ProcessType::Graphics,
                });
            }
        }
    }
    
    // Sort by memory usage descending
    gpu.process_list.sort_by(|a, b| b.used_memory.cmp(&a.used_memory));
}

/// Get process name from PID
fn get_process_name(pid: u32) -> String {
    // Try to read from /proc/<pid>/comm
    if let Ok(name) = std::fs::read_to_string(format!("/proc/{}/comm", pid)) {
        return name.trim().to_string();
    }
    
    // Fallback: try cmdline
    if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{}/cmdline", pid)) {
        if let Some(cmd) = cmdline.split('\0').next() {
            if let Some(name) = cmd.rsplit('/').next() {
                return name.to_string();
            }
        }
    }
    
    format!("pid:{}", pid)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn test_collector_creation() {
        let collector = NvmlCollector::new(Duration::from_secs(5));
        // Just verify it doesn't panic
        let _ = collector.collect_all();
    }
    // VER-HEALTH-001/002/003: permanent public-exported driver-free native fixture.
    // Only tests spawn cc; production collection never invokes a subprocess.
    static HEALTH_FIXTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    pub(crate) fn health_definition_fixture_lock() -> std::sync::MutexGuard<'static,()> {
        let guard=HEALTH_FIXTURE_LOCK.lock().unwrap_or_else(|poisoned|poisoned.into_inner());
        // Test-global provider controls never leak between independently scheduled cases.
        for key in ["DCXP_FIXTURE_RECOVERY","DCXP_FIXTURE_QUERY","DCXP_FIXTURE_ERROR","DCXP_FIXTURE_TAG","DCXP_FIXTURE_NESTED_ERROR","DCXP_FIXTURE_BAD_FIELD","DCXP_FIXTURE_LINKS","DCXP_FIXTURE_MODEL","DCXP_FIXTURE_UNTOUCHED"] {std::env::remove_var(key);}
        guard
    }
    pub(crate) fn health_definition_fixture_path() -> &'static std::path::PathBuf {
        static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        PATH.get_or_init(|| {
            let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let dir=std::env::temp_dir().join(format!("dcxp-health-native-fixture-{}-{nonce}",std::process::id()));
            std::fs::create_dir(&dir).expect("create unique owned test fixture directory");
            let source=dir.join("fixture.c");let library=dir.join("libdcxp-health-nvml.so");
            std::fs::write(&source,HEALTH_NATIVE_FIXTURE).expect("write inline synthetic test provider");
            let output=std::process::Command::new("cc").args(["-std=c11","-Wall","-Wextra","-Werror","-shared","-fPIC"])
                .arg(&source).arg("-o").arg(&library).output().expect("test C compiler must be available");
            assert!(output.status.success(),"native test fixture compilation: {}",String::from_utf8_lossy(&output.stderr));
            library
        })
    }
    pub(crate) fn health_definition_other_fixture_path(missing:bool) -> &'static std::path::PathBuf {
        static OTHER: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        static MISSING: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        let slot=if missing {&MISSING}else{&OTHER};
        slot.get_or_init(|| {
            let first=health_definition_fixture_path();let directory=first.parent().unwrap();
            let library=directory.join(if missing {"libdcxp-health-nvml-nofield.so"}else{"libdcxp-health-nvml-other.so"});
            let mut command=std::process::Command::new("cc");
            command.args(["-std=c11","-Wall","-Wextra","-Werror","-shared","-fPIC"]);
            if missing {command.arg("-DDCXP_FIXTURE_HIDE_FIELD");}
            let output=command.arg(directory.join("fixture.c")).arg("-o").arg(&library).output().expect("test C compiler");
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));library
        })
    }
    const HEALTH_NATIVE_FIXTURE: &str = r###"/* VER-HEALTH-001/002/003: software-only synthetic NVML provider.
 * ABI copied by declaration from locked nvml-wrapper-sys0.8.0/bindings.rs;
 * no hardware, syscalls, subprocesses, tenant data, or real driver library.
 * Fixture controls are consumed only by this test provider, never production. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stddef.h>
typedef void *Device;
typedef struct { char old[16]; unsigned domain,bus,device,id,subsystem; char busid[32]; } Pci;
typedef struct { uint64_t total,free,used; } Memory;
typedef union { double d; unsigned ui; unsigned long ul; unsigned long long ull; long long sll; } Value;
typedef struct { unsigned field,scope; long long timestamp,latency; unsigned tag,status; Value value; } Field;
_Static_assert(sizeof(Pci)==68,"pinned PCI ABI");
_Static_assert(sizeof(Field)==40 && offsetof(Field,value)==32,"pinned field ABI");
static unsigned idx(Device d) { return (unsigned)(uintptr_t)d-1; }
static const char *setting(const char *name,const char *fallback) { const char *s=getenv(name); return s?s:fallback; }
static unsigned failure(Device d,const char *query) {
  if(idx(d)==3 && !strcmp(setting("DCXP_FIXTURE_QUERY",""),query)) return strtoul(setting("DCXP_FIXTURE_ERROR","999"),0,10);
  return 0;
}
static unsigned init_count,shutdown_count,field_count;
unsigned dcxp_fixture_init_count(void) { return init_count; }
unsigned dcxp_fixture_shutdown_count(void) { return shutdown_count; }
unsigned dcxp_fixture_field_count(void) { return field_count; }
unsigned nvmlInit_v2(void) { ++init_count;return 0; }
unsigned nvmlShutdown(void) { ++shutdown_count;return 0; }
unsigned nvmlSystemGetDriverVersion(char *s,unsigned n) { snprintf(s,n,"615.71.09");return 0; }
unsigned nvmlDeviceGetCount_v2(unsigned *n) { *n=8;return 0; }
unsigned nvmlDeviceGetHandleByIndex_v2(unsigned i,Device *d) { unsigned e=failure((Device)(uintptr_t)(i+1),"device_by_index");if(e)return e;*d=(Device)(uintptr_t)(i+1);return 0; }
unsigned nvmlDeviceGetName(Device d,char *s,unsigned n) { unsigned e=failure(d,"name");snprintf(s,n,"%s",setting("DCXP_FIXTURE_MODEL","NVIDIA L40S synthetic"));return e; }
unsigned nvmlDeviceGetUUID(Device d,char *s,unsigned n) { unsigned e=failure(d,"uuid");snprintf(s,n,"GPU-SYNTHETIC-%u",idx(d));return e; }
unsigned nvmlDeviceGetPciInfo_v3(Device d,Pci *p) { memset(p,0,sizeof(*p));p->bus=0x50+idx(d);p->id=0x26b510de;snprintf(p->busid,sizeof(p->busid),"00000000:%02x:00.0",p->bus);return failure(d,"pci_info"); }
unsigned nvmlDeviceGetTemperature(Device d,unsigned sensor,unsigned *v) { (void)sensor;*v=44;return failure(d,"temperature"); }
unsigned nvmlDeviceGetMemoryInfo(Device d,Memory *m) { m->total=48ULL<<30;m->used=1ULL<<30;m->free=m->total-m->used;return failure(d,"memory"); }
unsigned nvmlDeviceGetEccMode(Device d,unsigned *a,unsigned *b) { *a=*b=0;return failure(d,"ecc_mode"); }
unsigned nvmlDeviceGetTotalEccErrors(Device d,unsigned kind,unsigned counter,unsigned long long *v) { (void)counter;*v=0;return failure(d,kind?"ecc_uncorrected":"ecc_corrected"); }
#define UQUERY(fn,key,val) unsigned fn(Device d,unsigned *v) { *v=val;return failure(d,key); }
UQUERY(nvmlDeviceGetPowerUsage,"power_usage",120000)
UQUERY(nvmlDeviceGetEnforcedPowerLimit,"power_limit",350000)
UQUERY(nvmlDeviceGetPcieSpeed,"pcie_link_speed",32000)
unsigned nvmlDeviceGetPowerManagementLimitConstraints(Device d,unsigned *a,unsigned *b) { *a=100000;*b=350000;return failure(d,"power_constraints"); }
unsigned nvmlDeviceGetClockInfo(Device d,unsigned clock,unsigned *v) { *v=1400;return failure(d,clock==1?"sm_clock":"memory_clock"); }
unsigned nvmlDeviceGetUtilizationRates(Device d,unsigned *v) { v[0]=v[1]=3;return failure(d,"utilization"); }
unsigned nvmlDeviceGetEncoderUtilization(Device d,unsigned *a,unsigned *b) { *a=0;*b=1000;return failure(d,"encoder_utilization"); }
unsigned nvmlDeviceGetDecoderUtilization(Device d,unsigned *a,unsigned *b) { *a=0;*b=1000;return failure(d,"decoder_utilization"); }
unsigned nvmlDeviceGetFanSpeed_v2(Device d,unsigned fan,unsigned *v) { (void)fan;*v=30;return failure(d,"fan_speed"); }
unsigned nvmlDeviceGetCurrentClocksThrottleReasons(Device d,unsigned long long *v) { *v=0;return failure(d,"throttle_reasons"); }
unsigned nvmlDeviceGetPcieThroughput(Device d,unsigned direction,unsigned *v) { *v=0;return failure(d,direction==0?"pcie_tx":"pcie_rx"); }
unsigned nvmlDeviceGetNvLinkState(Device d,unsigned link,unsigned *v) { unsigned e=failure(d,"nvlink_state");*v=1; if(e)return e;return link<strtoul(setting("DCXP_FIXTURE_LINKS","0"),0,10)?0:2; }
unsigned nvmlDeviceGetComputeRunningProcesses_v3(Device d,unsigned *n,void *p) { (void)p;*n=0;return failure(d,"compute_processes"); }
unsigned nvmlDeviceGetGraphicsRunningProcesses_v3(Device d,unsigned *n,void *p) { (void)p;*n=0;return failure(d,"graphics_processes"); }
unsigned nvmlDeviceGetCudaComputeCapability(Device d,int *major,int *minor) { *major=8;*minor=9;return failure(d,"compute_capability"); }
#ifndef DCXP_FIXTURE_HIDE_FIELD
unsigned nvmlDeviceGetFieldValues(Device d,int n,Field *fields) {
 ++field_count;
 if(!strcmp(setting("DCXP_FIXTURE_UNTOUCHED",""),"yes"))return 0;
 unsigned call=failure(d,"recovery_action"); if(call)return call;
 for(int i=0;i<n;i++) {
  fields[i].timestamp=1000000;fields[i].latency=1;fields[i].status=0;fields[i].tag=1;fields[i].value.ui=0;
  if(idx(d)==3) {
   fields[i].tag=strtoul(setting("DCXP_FIXTURE_TAG","1"),0,10);
   fields[i].status=strtoul(setting("DCXP_FIXTURE_NESTED_ERROR","0"),0,10);
   if(fields[i].tag==0)fields[i].value.d=strtod(setting("DCXP_FIXTURE_RECOVERY","0"),0);
   else fields[i].value.ull=strtoull(setting("DCXP_FIXTURE_RECOVERY","0"),0,10);
   if(!strcmp(setting("DCXP_FIXTURE_BAD_FIELD",""),"yes"))fields[i].field=231;
  }
 }
 return 0;
}

#endif
"###;
// Appended only inside a snapshot of gpu.rs tests by run_definition.py.
// VER-HEALTH-001/002/003: exercise actual wrapper -> production NVML collector.
    fn health_fixture(reset: &str, query: &str, error: u32) -> Vec<GpuMetrics> {
        for key in ["DCXP_FIXTURE_TAG","DCXP_FIXTURE_NESTED_ERROR","DCXP_FIXTURE_BAD_FIELD","DCXP_FIXTURE_LINKS","DCXP_FIXTURE_MODEL"] {std::env::remove_var(key);}
        std::env::set_var("DCXP_FIXTURE_RECOVERY",reset);
        std::env::set_var("DCXP_FIXTURE_QUERY",query);
        std::env::set_var("DCXP_FIXTURE_ERROR",error.to_string());
        let fixture_path=health_definition_fixture_path();
        let lib=fixture_path.as_os_str();
        let context=NvmlContext::for_test(lib).expect("driver-free fixture must load");
        collect_metrics_from_nvml(&context).gpus
    }

    #[test]
    fn health_definition_native_reset_with_readable_basics_and_failed_power_is_error() {
        let _guard=health_definition_fixture_lock();
        let gpus=health_fixture("1","power_usage",999);
        assert_eq!(gpus.len(),8);
        assert_eq!(gpus[3].uuid,"GPU-SYNTHETIC-3");
        assert_eq!(gpus[3].pci_bus_id,"0000:53:00.0");
        assert_eq!(gpus[3].state.as_u8(),2,"native current Reset cannot look healthy");
        assert_eq!(super::super::collector::count_usable_gpus(&gpus),7);
        let output=super::super::prometheus::PrometheusFormatter::format(&super::super::AllMetrics {hostname:"fixture-host".into(),gpus,..Default::default()});
        let states:Vec<_>=output.lines().filter(|l|l.starts_with("GPU_ERROR_STATE{")).collect();
        assert_eq!(states.len(),8);
        assert!(states.iter().any(|l|l.contains("GPU-SYNTHETIC-3") && l.ends_with(" 2")));
        assert!(!output.lines().any(|l|l.starts_with("DCGM_FI_DEV_POWER_USAGE{") && l.contains("GPU-SYNTHETIC-3")));
    }

    #[test]
    fn health_definition_native_nonzero_known_and_future_recovery_codes_are_errors() {
        let _guard=health_definition_fixture_lock();
        for code in [1,2,3,4,5,6,7,42] {
            assert_eq!(health_fixture(&code.to_string(),"",0)[3].state.as_u8(),2,"code {code}");
        }
        assert_eq!(health_fixture("0","",0)[3].state.as_u8(),1);
    }

    #[test]
    fn health_definition_native_fatal_15_16_override_every_existing_device_query() {
        let _guard=health_definition_fixture_lock();
        // Each query names an actual native fixture function/argument branch.
        let mut failures=Vec::new();
        for query in ["name","uuid","pci_info","temperature","memory","ecc_mode","ecc_corrected","ecc_uncorrected",
            "power_usage","power_limit","power_constraints","sm_clock","memory_clock","utilization",
            "encoder_utilization","decoder_utilization","throttle_reasons","pcie_tx","pcie_rx","pcie_link_speed",
            "nvlink_state","compute_processes","graphics_processes","recovery_action"] {
            for code in [15,16] {
                let gpu=health_fixture("0",query,code).remove(3);
                if gpu.state.as_u8()!=2 { failures.push(format!("{query} native{code} state{}",gpu.state.as_u8())); }
                if query!="uuid" && gpu.uuid!="GPU-SYNTHETIC-3" { failures.push(format!("{query} native{code} lost available UUID")); }
                if query=="uuid" && !gpu.uuid.is_empty() { failures.push(format!("{query} native{code} invented UUID {}",gpu.uuid)); }
            }
        }
        assert!(failures.is_empty(),"All48 native fatal query cases inspected: {failures:?}");
    }

    #[test]
    fn health_definition_native_early_name_failure_keeps_available_uuid_and_bdf() {
        let _guard=health_definition_fixture_lock();
        let gpu=health_fixture("0","name",15).remove(3);
        assert_eq!(gpu.state.as_u8(),2);
        assert_eq!(gpu.uuid,"GPU-SYNTHETIC-3");
        assert_eq!(gpu.pci_bus_id,"0000:53:00.0");
    }

    #[test]
    fn health_definition_native_optional_unknown_is_zero_but_unsupported_is_coverage() {
        let _guard=health_definition_fixture_lock();
        for code in [2,4,6,999] {
            assert_eq!(health_fixture("0","power_usage",code)[3].state.as_u8(),0,"unexpected optional native {code}");
        }
        for code in [3,13] {
            assert_eq!(health_fixture("0","power_usage",code)[3].state.as_u8(),1,"unsupported native {code}");
        }
    }

    #[test]
    fn health_definition_native_recovery_nested_and_numeric_invalidity_is_unknown() {
        let _guard=health_definition_fixture_lock();
        for (tag,value) in [(0,"-1"),(0,"1.5"),(0,"NaN"),(0,"inf"),(0,"4294967296"),(2,"4294967296"),(3,"4294967296"),(4,"18446744073709551615"),(99,"0")] {
            health_fixture("0","",0);
            std::env::set_var("DCXP_FIXTURE_TAG",tag.to_string());std::env::set_var("DCXP_FIXTURE_RECOVERY",value);
            let context=NvmlContext::for_test(health_definition_fixture_path().as_os_str()).unwrap();
            let gpu=collect_metrics_from_nvml(&context).gpus.remove(3);
            assert_eq!(gpu.state.as_u8(),0,"tag {tag} value {value}");
        }
        for code in [15,16,999] {
            health_fixture("0","",0);std::env::set_var("DCXP_FIXTURE_NESTED_ERROR",code.to_string());
            let context=NvmlContext::for_test(health_definition_fixture_path().as_os_str()).unwrap();
            assert_eq!(collect_metrics_from_nvml(&context).gpus[3].state.as_u8(),if code==999 {0}else{2});
        }
    }

    #[test]
    fn health_definition_native_twelve_of_eighteen_nvlink_slots_is_nominal() {
        let _guard=health_definition_fixture_lock();
        health_fixture("0","",0);
        std::env::set_var("DCXP_FIXTURE_MODEL","NVIDIA A100 synthetic");std::env::set_var("DCXP_FIXTURE_LINKS","12");
        let context=NvmlContext::for_test(health_definition_fixture_path().as_os_str()).unwrap();
        let gpu=collect_metrics_from_nvml(&context).gpus.remove(3);
        assert_eq!(gpu.state.as_u8(),1);assert_eq!(gpu.nvlink_active,12);
        std::env::set_var("DCXP_FIXTURE_MODEL","NVIDIA L40S synthetic");std::env::set_var("DCXP_FIXTURE_LINKS","0");
        assert_eq!(collect_metrics_from_nvml(&context).gpus[3].state.as_u8(),1);
        std::env::set_var("DCXP_FIXTURE_MODEL","NVIDIA L4");
        assert_eq!(collect_metrics_from_nvml(&context).gpus[3].state.as_u8(),1,"exact L4 model has no NvLink");
    }

    #[test]
    fn health_definition_native_architecture_conditional_fatal_queries_are_errors() {
        let _guard=health_definition_fixture_lock();
        for query in ["fan_speed","compute_capability"] {
            for code in [15,16] {
                health_fixture("0",query,code);
                std::env::set_var("DCXP_FIXTURE_MODEL","Synthetic unknown architecture");
                let context=NvmlContext::for_test(health_definition_fixture_path().as_os_str()).unwrap();
                assert_eq!(collect_metrics_from_nvml(&context).gpus[3].state.as_u8(),2,"{query} {code}");
            }
        }
    }

    #[test]
    fn health_definition_native_wrapper_ulong_fixture_pins_loss_before_checked_decoder() {
        let _guard=health_definition_fixture_lock();
        health_fixture("4294967296","",0);std::env::set_var("DCXP_FIXTURE_TAG","2");
        let context=NvmlContext::for_test(health_definition_fixture_path().as_os_str()).unwrap();
        let device=context.nvml.device_by_index(3).unwrap();
        let samples=device.field_values_for(&[nvml_wrapper::structs::device::FieldId(230)]).unwrap();
        assert!(matches!(&samples[0].as_ref().unwrap().value,Ok(nvml_wrapper::enums::device::SampleValue::U32(0))),"pinned0.10 wrapper truncation must be accounted for before decode");
    }

// VER-HEALTH-001/002: candidate raw-preserving adapter API, intentionally absent.
// Tests preserve native raw metadata before the locked wrapper can truncate values.
    use nvml_wrapper::error::NvmlError;
    fn health_definition_context_fixture() -> NvmlContext {
        NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_fixture_path().as_os_str()).unwrap()
    }
    #[test]
    fn health_definition_future_raw_adapter_validates_call_field_metadata_and_nested_native_returns() {
        let _guard=health_definition_fixture_lock();
        let mut outcomes=Vec::new();
        for (key,value,expected) in [("DCXP_FIXTURE_QUERY","recovery_action",0),("DCXP_FIXTURE_BAD_FIELD","yes",0),
            ("DCXP_FIXTURE_TAG","99",0),("DCXP_FIXTURE_NESTED_ERROR","999",0),("DCXP_FIXTURE_NESTED_ERROR","15",2),
            ("DCXP_FIXTURE_NESTED_ERROR","16",2),("DCXP_FIXTURE_UNTOUCHED","yes",0)] {
            for key in ["DCXP_FIXTURE_QUERY","DCXP_FIXTURE_BAD_FIELD","DCXP_FIXTURE_TAG","DCXP_FIXTURE_NESTED_ERROR","DCXP_FIXTURE_UNTOUCHED"] {std::env::remove_var(key);}
            std::env::set_var("DCXP_FIXTURE_RECOVERY","0");std::env::set_var("DCXP_FIXTURE_ERROR","999");std::env::set_var(key,value);
            let context=health_definition_context_fixture();let gpu=collect_metrics_from_nvml(&context).gpus.remove(3);
            outcomes.push((key,value,gpu.state.as_u8(),expected));
        }
        assert!(outcomes.iter().all(|(_,_,actual,expected)|actual==expected),"{outcomes:?}");
        std::env::remove_var("DCXP_FIXTURE_UNTOUCHED");
    }
    #[test]
    fn health_definition_future_native_numeric_decoder_rejects_ulong_overflow_before_wrapper_cast() {
        let _guard=health_definition_fixture_lock();
        // Native value tags0double/1uint/2ulong/3ulonglong/4signedlonglong.
        // This seam must receive raw tag/union bits, before SampleValue::U32 truncation.
        for tag in [1,2,3,4,5] {
            assert_eq!(decode_native_recovery_value(tag,7),Some(7));
        }
        for tag in [2,3] {assert_eq!(decode_native_recovery_value(tag,1u64<<32),None);}
        assert_eq!(decode_native_recovery_value(4,u64::MAX),None);
        assert_eq!(decode_native_recovery_value(5,u32::MAX as u64),None);
        for v in [-1.0,1.5,f64::NAN,f64::INFINITY,4294967296.0] {assert_eq!(decode_native_recovery_value(0,v.to_bits()),None);}
        assert_eq!(decode_native_recovery_value(0,7.0f64.to_bits()),Some(7));
        assert_eq!(decode_native_recovery_value(99,0),None);
    }
    #[test]
    fn health_definition_future_wrapper_payload_is_never_invented_native_return_code() {
        let _guard=health_definition_fixture_lock();
        let mut gpu=GpuMetrics::default();gpu.uuid="GPU-SYNTHETIC".into();
        record_nvml_query_error(&mut gpu,NvmlQuery::PowerUsage,&NvmlError::UnexpectedVariant(15));
        let output=crate::metrics::prometheus::PrometheusFormatter::format(&crate::metrics::AllMetrics {gpus:vec![gpu],..Default::default()});
        let line=output.lines().find(|l|l.starts_with("DCXP_GPU_NVML_ERROR{")).unwrap();
        assert!(line.contains("query=\"power_usage\""));assert!(line.contains("error=\"conversion_error\""));assert!(line.contains("code=\"unknown\""));
        assert!(!line.contains("code=\"15\""));
    }
    #[test]
    fn health_definition_future_symbol_payload_is_never_exported() {
        let _guard=health_definition_fixture_lock();
        let mut gpu=GpuMetrics::default();
        record_nvml_query_error(&mut gpu,NvmlQuery::PowerUsage,&NvmlError::FailedToLoadSymbol("SYNTHETIC_PRIVATE_PATH_DO_NOT_EXPORT".into()));
        let output=crate::metrics::prometheus::PrometheusFormatter::format(&crate::metrics::AllMetrics {gpus:vec![gpu],..Default::default()});
        assert!(!output.contains("SYNTHETIC_PRIVATE_PATH_DO_NOT_EXPORT"));
        assert!(output.lines().any(|l|l.starts_with("DCXP_GPU_NVML_ERROR{") && l.contains("error=\"symbol_unavailable\"") && l.contains("code=\"unknown\"")));
    }
    // Private proposed context API agreed by root/Astra. Test paths cfg(test) only.
    struct HealthFixtureLibrary(*mut libc::c_void);
    impl HealthFixtureLibrary {
        fn open(path:&str)->Self {
            let path=std::ffi::CString::new(path).unwrap();
            let ptr=unsafe {libc::dlopen(path.as_ptr(),libc::RTLD_NOW|libc::RTLD_LOCAL)};
            assert!(!ptr.is_null());Self(ptr)
        }
        fn count(&self,symbol:&str)->u32 {
            let symbol=std::ffi::CString::new(symbol).unwrap();
            let ptr=unsafe {libc::dlsym(self.0,symbol.as_ptr())};assert!(!ptr.is_null());
            let fun:unsafe extern "C" fn()->u32=unsafe {std::mem::transmute(ptr)};
            unsafe {fun()}
        }
    }
    impl Drop for HealthFixtureLibrary {fn drop(&mut self) {assert_eq!(unsafe {libc::dlclose(self.0)},0);}}
    #[test]
    fn health_definition_future_raw_field_abi_matches_locked_lp64_bindings() {
        let _guard=health_definition_fixture_lock();
        use std::mem::{size_of,align_of,offset_of};
        assert_eq!(size_of::<RawFieldValue>(),40);assert_eq!(align_of::<RawFieldValue>(),8);
        assert_eq!(size_of::<RawNvmlValue>(),8);assert_eq!(align_of::<RawNvmlValue>(),8);
        assert_eq!(offset_of!(RawFieldValue,field_id),0);assert_eq!(offset_of!(RawFieldValue,scope_id),4);
        assert_eq!(offset_of!(RawFieldValue,timestamp),8);assert_eq!(offset_of!(RawFieldValue,latency_us),16);
        assert_eq!(offset_of!(RawFieldValue,value_type),24);assert_eq!(offset_of!(RawFieldValue,nvml_return),28);
        assert_eq!(offset_of!(RawFieldValue,value),32);
    }
    #[test]
    fn health_definition_future_context_same_library_positive_and_wrong_owner_zero_calls() {
        let _guard=health_definition_fixture_lock();
        std::env::set_var("DCXP_FIXTURE_RECOVERY","1");
        for key in ["DCXP_FIXTURE_TAG","DCXP_FIXTURE_QUERY","DCXP_FIXTURE_NESTED_ERROR","DCXP_FIXTURE_UNTOUCHED"] {std::env::remove_var(key);}
        let first=HealthFixtureLibrary::open(crate::metrics::gpu::tests::health_definition_fixture_path().to_str().unwrap());
        let second=HealthFixtureLibrary::open(crate::metrics::gpu::tests::health_definition_other_fixture_path(false).to_str().unwrap());
        let context_a=NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_fixture_path().as_os_str()).unwrap();
        let context_b=NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_other_fixture_path(false).as_os_str()).unwrap();
        let device_a=context_a.nvml.device_by_index(3).unwrap();let device_b=context_b.nvml.device_by_index(3).unwrap();
        let gpu=collect_metrics_from_nvml(&context_a).gpus.remove(3);
        assert_eq!(gpu.state.as_u8(),2,"same provider Reset must actually be decoded");
        assert_eq!(context_a.recovery_observation(&device_a).action,Some(1));
        let before_a=first.count("dcxp_fixture_field_count");let before_b=second.count("dcxp_fixture_field_count");
        let rejected=context_a.recovery_observation(&device_b);
        assert_eq!(rejected.status.as_str(),"unavailable");assert_eq!(rejected.action,None);
        assert_eq!(first.count("dcxp_fixture_field_count"),before_a);
        assert_eq!(second.count("dcxp_fixture_field_count"),before_b);
    }
    #[test]
    fn health_definition_future_context_has_one_init_shutdown_and_no_synthetic_zero() {
        let _guard=health_definition_fixture_lock();
        let keeper=HealthFixtureLibrary::open(crate::metrics::gpu::tests::health_definition_fixture_path().to_str().unwrap());
        let init_before=keeper.count("dcxp_fixture_init_count");let shutdown_before=keeper.count("dcxp_fixture_shutdown_count");
        for _ in 0..3 {
            let context=NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_fixture_path().as_os_str()).unwrap();
            let device=context.nvml.device_by_index(3).unwrap();
            std::env::set_var("DCXP_FIXTURE_UNTOUCHED","yes");
            let observation=context.recovery_observation(&device);
            assert_eq!(observation.action,None,"success without writing output cannot create valid0");
            let gpu=collect_metrics_from_nvml(&context).gpus.remove(3);
            assert_eq!(gpu.state.as_u8(),0,"success without writing output is invalid/unavailable, not valid0");
            assert!(gpu.nvml_errors.iter().filter(|e|e.query=="recovery_action").all(|e|e.code.is_none()),"untouched output must not invent native999");
            std::env::remove_var("DCXP_FIXTURE_UNTOUCHED");
        }
        assert_eq!(keeper.count("dcxp_fixture_init_count")-init_before,3);
        assert_eq!(keeper.count("dcxp_fixture_shutdown_count")-shutdown_before,3);
        assert!(NvmlContext::for_test(std::ffi::OsStr::new("/target/definitely-no-such-health-fixture.so")).is_err());
        let missing=NvmlContext::for_test(crate::metrics::gpu::tests::health_definition_other_fixture_path(true).as_os_str()).unwrap();
        let device=missing.nvml.device_by_index(3).unwrap();
        let observation=missing.recovery_observation(&device);assert_eq!(observation.status.as_str(),"unsupported");
        let gpu=collect_metrics_from_nvml(&missing).gpus.remove(3);
        let output=crate::metrics::prometheus::PrometheusFormatter::format(&crate::metrics::AllMetrics {gpus:vec![gpu],..Default::default()});
        assert!(output.lines().any(|l|l.starts_with("DCXP_GPU_RECOVERY_ACTION_STATUS{") && l.contains("status=\"unsupported\"")));
    }

    #[test]
    fn health_definition_nvlink_invalid_in_range_slot_is_unknown_not_unsupported() {
        let _guard=health_definition_fixture_lock();
        health_fixture("0","nvlink_state",2);
        std::env::set_var("DCXP_FIXTURE_MODEL","NVIDIA A100 synthetic");std::env::set_var("DCXP_FIXTURE_LINKS","12");
        let context=NvmlContext::for_test(health_definition_fixture_path().as_os_str()).unwrap();
        assert_eq!(collect_metrics_from_nvml(&context).gpus[3].state.as_u8(),0,"invalid handle/in-range InvalidArg is not proven unsupported coverage");
        std::env::set_var("DCXP_FIXTURE_ERROR","6");
        assert_eq!(collect_metrics_from_nvml(&context).gpus[3].state.as_u8(),0,"is_active has no documented blanket NotFound exemption");
    }

}
