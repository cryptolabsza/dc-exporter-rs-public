#[path = "../src/metrics/nvidia_rm/mod.rs"]
mod nvidia_rm;

use nvidia_rm::abi::*;
use std::mem::{align_of, offset_of, size_of};

#[test]
fn exact_590_shared_page_layout() {
    assert_eq!(RUSD_SIZE, 3144);
    assert_eq!(MEMORY_TIMESTAMP_OFFSET, 2328);
    assert_eq!(MEMORY_VALUE_OFFSET, 2336);
    assert_eq!(THERMAL_POLL_MASK, 0x10);
    assert_eq!(mapping_length(4096), Some(4096));
    assert_eq!(mapping_length(8192), Some(8192));
    assert_eq!(mapping_length(0), None);
    assert_eq!(mapping_length(3), None);
}

#[test]
fn rm_ioctl_records_match_c_x86_64_layout() {
    assert_eq!((size_of::<Alloc>(), align_of::<Alloc>()), (32, 8));
    assert_eq!(offset_of!(Alloc, params), 16);
    assert_eq!(offset_of!(Alloc, status), 28);
    assert_eq!(size_of::<Control>(), 32);
    assert_eq!(offset_of!(Control, params), 16);
    assert_eq!(size_of::<Free>(), 16);
    assert_eq!(size_of::<MapMemory>(), 48);
    assert_eq!(offset_of!(MapMemory, offset), 16);
    assert_eq!(size_of::<MapMemoryFd>(), 56);
    assert_eq!(offset_of!(MapMemoryFd, fd), 48);
    assert_eq!(size_of::<DeviceAlloc>(), 56);
    assert_eq!(size_of::<ClientAlloc>(), 120);
    assert_eq!(size_of::<PciInfo>(), 12);
    assert_eq!(size_of::<CardInfo>(), 72);
    assert_eq!(offset_of!(CardInfo, pci), 4);
    assert_eq!(offset_of!(CardInfo, minor), 56);
    assert_eq!(size_of::<AttachIds>(), 132);
    assert_eq!(size_of::<UuidInfo>(), 272);
}

#[test]
fn linux_ioctl_encodings_are_not_native_macos_encodings() {
    assert_eq!(linux_ioctl(0x2b, 32), 0xc020462b);
    assert_eq!(linux_ioctl(0x4e, 56), 0xc038464e);
    assert_eq!(linux_ioctl(201, 4), 0xc00446c9);
    assert_eq!(linux_ioctl(200, 72 * 32), 0xc90046c8);
}
