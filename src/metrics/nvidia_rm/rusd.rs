//! Sequence-safe memory sensor decoding and per-batch RM resource ownership.
use super::{abi::*, RmRequest, RmStatus};
use std::{collections::HashSet, time::Duration};

pub fn profile_supported(request: &RmRequest, os: &str, arch: &str) -> bool {
    os == "linux"
        && arch == "x86_64"
        && profile_for_driver(&request.driver_version).is_some()
        && request.device_id == 0x2b85
}

/// The independently read NVRM line must identify the request's exact reviewed ABI.
pub fn running_driver_matches(request_version: &str, proc_version_text: &str) -> bool {
    if profile_for_driver(request_version).is_none() {
        return false;
    }
    let mut lines = proc_version_text
        .lines()
        .filter_map(|line| line.strip_prefix("NVRM version:"));
    let Some(line) = lines.next() else {
        return false;
    };
    if lines.next().is_some() || !line.contains("Kernel Module") {
        return false;
    }
    let mut versions = line.split_whitespace().filter(|word| {
        word.as_bytes().first().is_some_and(u8::is_ascii_digit) && word.contains('.')
    });
    versions.next() == Some(request_version) && versions.next().is_none()
}

pub fn normalize_bdf(input: &str) -> Option<String> {
    let fields: Vec<_> = input.split(':').collect();
    if fields.len() != 3 || !matches!(fields[0].len(), 4 | 8) || fields[1].len() != 2 {
        return None;
    }
    let (slot, function) = fields[2].split_once('.')?;
    if slot.len() != 2
        || function.len() != 1
        || fields
            .iter()
            .take(2)
            .any(|s| !s.bytes().all(|b| b.is_ascii_hexdigit()))
        || !slot.bytes().all(|b| b.is_ascii_hexdigit())
        || !function.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let domain = u32::from_str_radix(fields[0], 16).ok()?;
    let bus = u8::from_str_radix(fields[1], 16).ok()?;
    let slot = u8::from_str_radix(slot, 16).ok()?;
    let function = u8::from_str_radix(function, 16).ok()?;
    if domain > 0xffff || slot > 31 || function > 7 {
        return None;
    }
    Some(format!("{domain:04x}:{bus:02x}:{slot:02x}.{function:x}"))
}

fn valid_uuid(uuid: &str) -> bool {
    let Some(value) = uuid.strip_prefix("GPU-") else {
        return false;
    };
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

pub fn validate_requests(requests: &[RmRequest]) -> Result<(), RmStatus> {
    if requests.len() > MAX_DEVICES {
        return Err(RmStatus::Invalid);
    }
    let mut bdfs = HashSet::new();
    let mut uuids = HashSet::new();
    for request in requests {
        let bdf = normalize_bdf(&request.pci_bus_id).ok_or(RmStatus::Invalid)?;
        if !valid_uuid(&request.uuid)
            || !bdfs.insert(bdf)
            || !uuids.insert(request.uuid.to_ascii_lowercase())
        {
            return Err(RmStatus::Invalid);
        }
    }
    Ok(())
}

pub struct DeviceIdentity {
    pub pci_bus_id: String,
    pub uuid: String,
    pub vendor_id: u16,
    pub device_id: u16,
    pub minor: u32,
    pub rm_minor: u32,
    pub bound_to_nvidia: bool,
}
pub fn verify_identity(request: &RmRequest, identity: &DeviceIdentity) -> Result<(), RmStatus> {
    if normalize_bdf(&request.pci_bus_id).is_none()
        || normalize_bdf(&request.pci_bus_id) != normalize_bdf(&identity.pci_bus_id)
        || !request.uuid.eq_ignore_ascii_case(&identity.uuid)
        || identity.vendor_id != 0x10de
        || identity.device_id != request.device_id
        || identity.minor != identity.rm_minor
        || !identity.bound_to_nvidia
    {
        Err(RmStatus::DeviceLost)
    } else {
        Ok(())
    }
}

/// Signed 24.8 Celsius. Reject out-of-profile temperatures; never clamp a sentinel.
pub fn decode_temperature(raw: i32) -> Result<f64, RmStatus> {
    let celsius = raw as f64 / 256.0;
    if !(0.0..=150.0).contains(&celsius) {
        Err(RmStatus::Invalid)
    } else {
        Ok(celsius)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapshot {
    pub marker: u64,
    pub celsius: f64,
}
fn valid_marker(marker: u64) -> bool {
    marker != 0 && marker != u64::MAX && (marker < SEQ_START || marker & 1 == 0)
}
pub fn stable_snapshot(mut read: impl FnMut() -> (u64, i32, u64)) -> Result<Snapshot, RmStatus> {
    for _ in 0..10 {
        let (before, raw, after) = read();
        if valid_marker(before) && before == after {
            return Ok(Snapshot {
                marker: before,
                celsius: decode_temperature(raw)?,
            });
        }
    }
    Err(RmStatus::Invalid)
}

/// Fixture decoder. Actual mapped reads use volatile aligned loads and CPU/compiler fences.
pub fn snapshot_bytes(bytes: &[u8]) -> Result<Snapshot, RmStatus> {
    snapshot_bytes_for_profile(
        bytes,
        profile_for_driver(DRIVER_VERSION).ok_or(RmStatus::Invalid)?,
    )
}

pub fn snapshot_bytes_for_profile(bytes: &[u8], profile: AbiProfile) -> Result<Snapshot, RmStatus> {
    if !profile.is_valid() || bytes.len() < profile.shared_size {
        return Err(RmStatus::Invalid);
    }
    let marker = u64::from_le_bytes(
        bytes[MEMORY_TIMESTAMP_OFFSET..MEMORY_TIMESTAMP_OFFSET + 8]
            .try_into()
            .map_err(|_| RmStatus::Invalid)?,
    );
    let raw = i32::from_le_bytes(
        bytes[MEMORY_VALUE_OFFSET..MEMORY_VALUE_OFFSET + 4]
            .try_into()
            .map_err(|_| RmStatus::Invalid)?,
    );
    stable_snapshot(|| (marker, raw, marker))
}

pub fn expire_completed(
    reading: &mut super::RmReading,
    observed: Duration,
    now: Duration,
    max_age: Duration,
) {
    if reading.status == RmStatus::Valid
        && !now.checked_sub(observed).is_some_and(|age| age <= max_age)
    {
        reading.status = RmStatus::Stale;
        reading.celsius = None;
    }
}

#[derive(Default)]
pub struct Freshness {
    marker: Option<u64>,
    last_progress: Option<Duration>,
}
impl Freshness {
    /// No cross-clock assumption: the first observed value is unavailable until progress.
    pub fn observe(&mut self, marker: u64, now: Duration, max_age: Duration) -> RmStatus {
        if !valid_marker(marker) {
            return RmStatus::Invalid;
        }
        match self.marker {
            None => {
                self.marker = Some(marker);
                return RmStatus::Stale;
            }
            Some(previous) if marker < previous => return RmStatus::Invalid,
            Some(previous) if marker != previous => {
                self.marker = Some(marker);
                self.last_progress = Some(now);
            }
            _ => {}
        }
        match self.last_progress {
            Some(progress) if now.checked_sub(progress).is_some_and(|age| age <= max_age) => {
                RmStatus::Valid
            }
            _ => RmStatus::Stale,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    OpenControl,
    AllocateClient,
    VerifyIdentity,
    AttachGpu,
    OpenDevice,
    RegisterDevice,
    AllocateDevice,
    AllocateSubdevice,
    AllocateShared,
    OpenMapping,
    MapMemory,
    MapReadOnly,
    PollThermal,
    Unmap,
    UnmapMemory,
    CloseMapping,
    FreeShared,
    FreeSubdevice,
    FreeDevice,
    CloseDevice,
    DetachGpu,
    FreeClient,
    CloseControl,
}
pub trait Transport {
    fn perform(&mut self, op: Operation) -> Result<(), RmStatus>;
}
impl<T: Transport> Transport for &mut T {
    fn perform(&mut self, op: Operation) -> Result<(), RmStatus> {
        (**self).perform(op)
    }
}
const INIT: &[Operation] = &[
    Operation::OpenControl,
    Operation::AllocateClient,
    Operation::VerifyIdentity,
    Operation::AttachGpu,
    Operation::OpenDevice,
    Operation::RegisterDevice,
    Operation::AllocateDevice,
    Operation::AllocateSubdevice,
    Operation::AllocateShared,
    Operation::OpenMapping,
    Operation::MapMemory,
    Operation::MapReadOnly,
    Operation::PollThermal,
];
fn cleanup(op: Operation) -> Option<Operation> {
    use Operation::*;
    match op {
        OpenControl => Some(CloseControl),
        AllocateClient => Some(FreeClient),
        AttachGpu => Some(DetachGpu),
        OpenDevice => Some(CloseDevice),
        AllocateDevice => Some(FreeDevice),
        AllocateSubdevice => Some(FreeSubdevice),
        AllocateShared => Some(FreeShared),
        OpenMapping => Some(CloseMapping),
        MapMemory => Some(UnmapMemory),
        MapReadOnly => Some(Unmap),
        _ => None,
    }
}

/// The native driver and mocks share this exact acquisition ledger and Drop cleanup.
struct Session<T: Transport> {
    transport: T,
    releases: Vec<Operation>,
}
impl<T: Transport> Session<T> {
    fn new(transport: T) -> Result<Self, RmStatus> {
        let mut session = Self {
            transport,
            releases: Vec::new(),
        };
        for &op in INIT {
            session.transport.perform(op)?;
            if let Some(release) = cleanup(op) {
                session.releases.push(release);
            }
        }
        Ok(session)
    }
}
impl<T: Transport> Drop for Session<T> {
    fn drop(&mut self) {
        for &op in self.releases.iter().rev() {
            let _ = self.transport.perform(op);
        }
    }
}
pub fn with_session<T: Transport, R>(
    transport: &mut T,
    run: impl FnOnce(&mut T) -> Result<R, RmStatus>,
) -> Result<R, RmStatus> {
    let mut session = Session::new(transport)?;
    run(&mut session.transport)
}
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(super) mod native {
    use super::super::RmReading;
    use super::*;
    use std::{
        fs::{self, File, OpenOptions},
        os::fd::AsRawFd,
        path::{Path, PathBuf},
        ptr::NonNull,
        sync::atomic::{compiler_fence, Ordering},
        time::Instant,
    };

    fn io_status(error: std::io::Error) -> RmStatus {
        match error.raw_os_error() {
            Some(libc::EACCES) | Some(libc::EPERM) => RmStatus::PermissionDenied,
            Some(libc::ENOENT) | Some(libc::ENODEV) | Some(libc::EIO) => RmStatus::DeviceLost,
            _ => RmStatus::Invalid,
        }
    }
    fn rm_status(status: u32) -> Result<(), RmStatus> {
        match status {
            0 => Ok(()),
            0x1b => Err(RmStatus::PermissionDenied),
            0x0f => Err(RmStatus::DeviceLost),
            _ => Err(RmStatus::Invalid),
        }
    }
    fn ioctl<T>(file: &File, nr: u32, value: &mut T) -> Result<(), RmStatus> {
        // SAFETY: repr(C) records have checked ABI sizes; file lives through the call.
        let result = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                linux_ioctl(nr, std::mem::size_of::<T>()),
                value as *mut T,
            )
        };
        if result < 0 {
            Err(io_status(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
    fn open(path: impl AsRef<Path>) -> Result<File, RmStatus> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(io_status)
    }
    fn bound(bdf: &str) -> bool {
        // Only readlink: never access driver_override or other locking sysfs attributes.
        fs::read_link(format!("/sys/bus/pci/devices/{bdf}/driver"))
            .ok()
            .is_some_and(|p| p.file_name().is_some_and(|n| n == "nvidia"))
    }
    fn hex_file(path: PathBuf) -> Result<u16, RmStatus> {
        let value = fs::read_to_string(path).map_err(io_status)?;
        u16::from_str_radix(value.trim().trim_start_matches("0x"), 16)
            .map_err(|_| RmStatus::Invalid)
    }

    struct NativeTransport {
        request: RmRequest,
        profile: AbiProfile,
        bdf: String,
        minor: u32,
        gpu_id: u32,
        device_instance: u32,
        subdevice_instance: u32,
        control: Option<File>,
        device: Option<File>,
        mapping: Option<File>,
        client: u32,
        device_handle: u32,
        subdevice_handle: u32,
        shared_handle: u32,
        mapped: Option<NonNull<u8>>,
        mapped_length: usize,
        map_address: u64,
    }
    impl NativeTransport {
        fn new(request: &RmRequest) -> Result<Self, RmStatus> {
            if !profile_supported(request, "linux", "x86_64") {
                return Err(RmStatus::Unsupported);
            }
            let profile =
                profile_for_driver(&request.driver_version).ok_or(RmStatus::Unsupported)?;
            let bdf = normalize_bdf(&request.pci_bus_id).ok_or(RmStatus::Invalid)?;
            if !bound(&bdf) {
                return Err(RmStatus::DeviceLost);
            }
            let sysfs = Path::new("/sys/bus/pci/devices").join(&bdf);
            if hex_file(sysfs.join("vendor"))? != 0x10de
                || hex_file(sysfs.join("device"))? != request.device_id
            {
                return Err(RmStatus::DeviceLost);
            }
            let version = fs::read_to_string("/proc/driver/nvidia/version").map_err(io_status)?;
            if !running_driver_matches(profile.driver_version, &version) {
                return Err(RmStatus::Unsupported);
            }
            let info = fs::read_to_string(format!("/proc/driver/nvidia/gpus/{bdf}/information"))
                .map_err(io_status)?;
            let field = |name: &str| {
                info.lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.trim() == name)
                    .map(|(_, value)| value.trim())
            };
            if field("Bus Location").and_then(normalize_bdf).as_deref() != Some(bdf.as_str())
                || !field("GPU UUID").is_some_and(|uuid| uuid.eq_ignore_ascii_case(&request.uuid))
            {
                return Err(RmStatus::DeviceLost);
            }
            let minor = field("Device Minor")
                .and_then(|s| s.parse::<u32>().ok())
                .filter(|&v| v < 256)
                .ok_or(RmStatus::Invalid)?;
            // SAFETY: sysconf has no pointer operands.
            let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            let mapped_length = usize::try_from(page_size)
                .ok()
                .and_then(|size| profile.mapping_length(size))
                .ok_or(RmStatus::Invalid)?;
            Ok(Self {
                request: request.clone(),
                profile,
                bdf,
                minor,
                gpu_id: 0,
                device_instance: 0,
                subdevice_instance: 0,
                control: None,
                device: None,
                mapping: None,
                client: 0xcb000001,
                device_handle: 0xcb000002,
                subdevice_handle: 0xcb000003,
                shared_handle: 0xcb000004,
                mapped: None,
                mapped_length,
                map_address: 0,
            })
        }
        fn ctl(&self) -> Result<&File, RmStatus> {
            self.control.as_ref().ok_or(RmStatus::Invalid)
        }
        fn alloc<T>(
            &self,
            root: u32,
            parent: u32,
            object: u32,
            class: u32,
            params: &mut T,
        ) -> Result<u32, RmStatus> {
            let mut value = Alloc {
                root,
                parent,
                object,
                class,
                params: params as *mut T as u64,
                params_size: std::mem::size_of::<T>() as u32,
                ..Default::default()
            };
            ioctl(self.ctl()?, 0x2b, &mut value)?;
            rm_status(value.status)?;
            Ok(value.object)
        }
        fn control<T>(&self, object: u32, command: u32, params: &mut T) -> Result<(), RmStatus> {
            let mut value = Control {
                client: self.client,
                object,
                command,
                params: params as *mut T as u64,
                params_size: std::mem::size_of::<T>() as u32,
                ..Default::default()
            };
            ioctl(self.ctl()?, 0x2a, &mut value)?;
            rm_status(value.status)
        }
        fn free(&self, parent: u32, object: u32) -> Result<(), RmStatus> {
            let mut value = Free {
                root: self.client,
                parent,
                object,
                ..Default::default()
            };
            ioctl(self.ctl()?, 0x29, &mut value)?;
            rm_status(value.status)
        }
        fn identity(&mut self) -> Result<(), RmStatus> {
            let mut cards = [CardInfo::default(); MAX_DEVICES];
            ioctl(self.ctl()?, 200, &mut cards)?;
            let mut matches = cards.iter().filter(|card| {
                card.valid != 0
                    && format!(
                        "{:04x}:{:02x}:{:02x}.{:x}",
                        card.pci.domain, card.pci.bus, card.pci.slot, card.pci.function
                    ) == self.bdf
            });
            let card = *matches.next().ok_or(RmStatus::DeviceLost)?;
            if matches.next().is_some() || card.minor != self.minor {
                return Err(RmStatus::DeviceLost);
            }
            let mut uuid = UuidInfo {
                uuid: [0; 256],
                flags: 0,
                gpu_id: 0,
                device_instance: 0,
                subdevice_instance: 0,
            };
            uuid.uuid[..self.request.uuid.len()].copy_from_slice(self.request.uuid.as_bytes());
            self.control(self.client, 0x274, &mut uuid)?;
            let mut id = IdInfo {
                gpu_id: card.gpu_id,
                ..Default::default()
            };
            self.control(self.client, 0x205, &mut id)?;
            if uuid.gpu_id != card.gpu_id
                || uuid.device_instance != id.device_instance
                || uuid.subdevice_instance != id.subdevice_instance
            {
                return Err(RmStatus::DeviceLost);
            }
            // RM deviceInstance selects the device object, not the /dev node minor.
            verify_identity(
                &self.request,
                &DeviceIdentity {
                    pci_bus_id: self.bdf.clone(),
                    uuid: self.request.uuid.clone(),
                    vendor_id: card.pci.vendor_id,
                    device_id: card.pci.device_id,
                    minor: card.minor,
                    rm_minor: self.minor,
                    bound_to_nvidia: bound(&self.bdf),
                },
            )?;
            self.gpu_id = card.gpu_id;
            self.device_instance = id.device_instance;
            self.subdevice_instance = id.subdevice_instance;
            Ok(())
        }
        fn snapshot(&self) -> Result<Snapshot, RmStatus> {
            if !bound(&self.bdf) {
                return Err(RmStatus::DeviceLost);
            }
            let base = self.mapped.ok_or(RmStatus::Invalid)?;
            if !self.profile.is_valid()
                || self.mapped_length < self.profile.shared_size
                || base.as_ptr() as usize % 8 != 0
            {
                return Err(RmStatus::Invalid);
            }
            stable_snapshot(|| {
                // SAFETY: a successful read-only mmap owns mapped_length bytes; offsets
                // are in bounds and aligned. The driver can mutate this shared page.
                unsafe {
                    let marker = base.as_ptr().add(MEMORY_TIMESTAMP_OFFSET).cast::<u64>();
                    let before = std::ptr::read_volatile(marker);
                    compiler_fence(Ordering::Acquire);
                    std::arch::x86_64::_mm_lfence();
                    let raw = std::ptr::read_volatile(
                        base.as_ptr().add(MEMORY_VALUE_OFFSET).cast::<i32>(),
                    );
                    compiler_fence(Ordering::Acquire);
                    std::arch::x86_64::_mm_lfence();
                    (before, raw, std::ptr::read_volatile(marker))
                }
            })
        }
    }

    impl Transport for NativeTransport {
        fn perform(&mut self, op: Operation) -> Result<(), RmStatus> {
            use Operation::*;
            match op {
                OpenControl => {
                    self.control = Some(open("/dev/nvidiactl")?);
                }
                AllocateClient => {
                    let mut params = ClientAlloc {
                        client: self.client,
                        process_id: std::process::id(),
                        process_name: [0; 100],
                        os_pid_info: 0,
                    };
                    params.process_name[..11].copy_from_slice(b"dc-exporter");
                    self.client = self.alloc(0, 0, self.client, 0x41, &mut params)?;
                }
                VerifyIdentity => self.identity()?,
                AttachGpu => {
                    if !bound(&self.bdf) {
                        return Err(RmStatus::DeviceLost);
                    }
                    let mut ids = AttachIds {
                        ids: [u32::MAX; MAX_DEVICES],
                        failed_id: u32::MAX,
                    };
                    ids.ids[0] = self.gpu_id;
                    self.control(self.client, 0x215, &mut ids)?;
                }
                OpenDevice => {
                    self.device = Some(open(format!("/dev/nvidia{}", self.minor))?);
                }
                RegisterDevice => {
                    let mut fd = self.ctl()?.as_raw_fd();
                    ioctl(self.device.as_ref().ok_or(RmStatus::Invalid)?, 201, &mut fd)?;
                }
                AllocateDevice => {
                    let mut p = DeviceAlloc {
                        device_id: self.device_instance,
                        ..Default::default()
                    };
                    self.device_handle =
                        self.alloc(self.client, self.client, self.device_handle, 0x80, &mut p)?;
                }
                AllocateSubdevice => {
                    let mut p = self.subdevice_instance;
                    self.subdevice_handle = self.alloc(
                        self.client,
                        self.device_handle,
                        self.subdevice_handle,
                        0x2080,
                        &mut p,
                    )?;
                }
                AllocateShared => {
                    let mut mask = THERMAL_POLL_MASK;
                    self.shared_handle = self.alloc(
                        self.client,
                        self.subdevice_handle,
                        self.shared_handle,
                        0xde,
                        &mut mask,
                    )?;
                }
                OpenMapping => {
                    self.mapping = Some(open("/dev/nvidiactl")?);
                }
                MapMemory => {
                    let mut p = MapMemoryFd {
                        params: super::super::abi::MapMemory {
                            client: self.client,
                            device: self.device_handle,
                            memory: self.shared_handle,
                            length: self.profile.shared_size as u64,
                            flags: 1,
                            ..Default::default()
                        },
                        fd: self.mapping.as_ref().ok_or(RmStatus::Invalid)?.as_raw_fd(),
                    };
                    ioctl(self.ctl()?, 0x4e, &mut p)?;
                    rm_status(p.params.status)?;
                    self.map_address = p.params.address;
                }
                MapReadOnly => {
                    // SAFETY: mapping fd was registered by successful RM_MAP_MEMORY.
                    let mapped = unsafe {
                        libc::mmap(
                            std::ptr::null_mut(),
                            self.mapped_length,
                            libc::PROT_READ,
                            libc::MAP_SHARED,
                            self.mapping.as_ref().ok_or(RmStatus::Invalid)?.as_raw_fd(),
                            0,
                        )
                    };
                    if mapped == libc::MAP_FAILED {
                        return Err(io_status(std::io::Error::last_os_error()));
                    }
                    self.mapped = NonNull::new(mapped.cast());
                    if self.mapped.is_none() {
                        unsafe { libc::munmap(mapped, self.mapped_length) };
                        return Err(RmStatus::Invalid);
                    }
                }
                PollThermal => {
                    let mut mask = THERMAL_POLL_MASK;
                    self.control(self.shared_handle, 0xde0001, &mut mask)?;
                }
                Unmap => {
                    if let Some(p) = self.mapped.take() {
                        unsafe { libc::munmap(p.as_ptr().cast(), self.mapped_length) };
                    }
                }
                UnmapMemory => {
                    let mut p = super::super::abi::UnmapMemory {
                        client: self.client,
                        device: self.device_handle,
                        memory: self.shared_handle,
                        address: self.map_address,
                        flags: 1,
                        ..Default::default()
                    };
                    ioctl(self.ctl()?, 0x4f, &mut p)?;
                    rm_status(p.status)?;
                }
                CloseMapping => {
                    self.mapping.take();
                }
                FreeShared => self.free(self.subdevice_handle, self.shared_handle)?,
                FreeSubdevice => self.free(self.device_handle, self.subdevice_handle)?,
                FreeDevice => self.free(self.client, self.device_handle)?,
                CloseDevice => {
                    self.device.take();
                }
                DetachGpu => {
                    let mut ids = DetachIds {
                        ids: [u32::MAX; MAX_DEVICES],
                    };
                    ids.ids[0] = self.gpu_id;
                    self.control(self.client, 0x216, &mut ids)?;
                }
                FreeClient => self.free(0, self.client)?,
                CloseControl => {
                    self.control.take();
                }
            }
            Ok(())
        }
    }

    pub fn collect(requests: &[RmRequest], timeout: Duration, max_age: Duration) -> Vec<RmReading> {
        let started = Instant::now();
        let mut results: Vec<_> = requests
            .iter()
            .map(|r| super::super::RmReading::unavailable(r, RmStatus::Stale))
            .collect();
        let mut sessions = Vec::new();
        let mut observed = vec![None; requests.len()];
        for (index, request) in requests.iter().enumerate() {
            if !profile_supported(request, "linux", "x86_64") {
                results[index].status = RmStatus::Unsupported;
                continue;
            }
            if started.elapsed() >= timeout {
                results[index].status = RmStatus::Timeout;
                continue;
            }
            match NativeTransport::new(request).and_then(Session::new) {
                Ok(session) => sessions.push((index, session, Freshness::default())),
                Err(status) => results[index].status = status,
            }
        }
        while started.elapsed() < timeout && !sessions.is_empty() {
            let mut completed = Vec::new();
            for (position, (index, session, freshness)) in sessions.iter_mut().enumerate() {
                match session.transport.snapshot() {
                    Ok(sample) => {
                        let status = freshness.observe(sample.marker, started.elapsed(), max_age);
                        results[*index].status = status;
                        if status == RmStatus::Valid {
                            results[*index].celsius = Some(sample.celsius);
                            observed[*index] = Some(started.elapsed());
                            completed.push(position);
                        } else if status == RmStatus::Invalid {
                            completed.push(position);
                        }
                    }
                    Err(status) => {
                        results[*index].status = status;
                        if status == RmStatus::DeviceLost {
                            completed.push(position);
                        }
                    }
                }
            }
            // Releases occur here for completed devices; pending devices share the wait.
            for position in completed.into_iter().rev() {
                sessions.swap_remove(position);
            }
            if !sessions.is_empty() {
                std::thread::sleep(
                    Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())),
                );
            }
        }
        // Drop releases all remaining mappings, objects, descriptors and attach references.
        drop(sessions);
        for (reading, observed) in results.iter_mut().zip(observed) {
            if let Some(observed) = observed {
                expire_completed(reading, observed, started.elapsed(), max_age);
            }
        }
        results
    }
}
