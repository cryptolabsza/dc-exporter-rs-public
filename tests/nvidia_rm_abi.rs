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

// REQ-THERMAL-005/009: complete sizes come from separate pinned NVIDIA headers.
#[test]
fn exact_profiles_select_distinct_shared_sizes_and_safe_page_rounding() {
    for (driver, size) in [
        ("590.48.01", 3144),
        ("595.99.02", 3152),
        ("615.71.09", 3184),
    ] {
        let profile = profile_for_driver(driver).expect("exact reviewed profile");
        assert_eq!(profile.driver_version, driver);
        assert_eq!(profile.shared_size, size);
        assert_eq!(profile.mapping_length(1), Some(size));
        assert_eq!(profile.mapping_length(8), Some(size));
        assert_eq!(profile.mapping_length(16), Some((size + 15) & !15));
        assert_eq!(profile.mapping_length(4096), Some(4096));
        assert_eq!(profile.mapping_length(8192), Some(8192));
        assert_eq!(profile.mapping_length(0), None);
        assert_eq!(profile.mapping_length(3), None);
        assert_eq!(profile.mapping_length(usize::MAX), None);
        let largest_page = 1usize << (usize::BITS - 1);
        assert_eq!(profile.mapping_length(largest_page), Some(largest_page));
    }
    assert_eq!(MEMORY_TIMESTAMP_OFFSET, 2328);
    assert_eq!(MEMORY_VALUE_OFFSET, 2336);
    assert_eq!(THERMAL_POLL_MASK, 0x10);
    for driver in [
        "",
        "590.48.00",
        "590.48.02",
        "595.99.01",
        "595.99.03",
        "595.99.02-open",
        "595.99.02 ",
        "0595.99.02",
        "600.00.00",
        "615.71.08",
        "615.71.10",
        "615.71.09-open",
        "615.71.09 ",
        "615.78.08",
        "999.00.00",
    ] {
        assert!(profile_for_driver(driver).is_none(), "{driver}");
    }
    for shared_size in [0, MEMORY_VALUE_OFFSET + 3, 3144, usize::MAX] {
        let malformed = AbiProfile {
            driver_version: "595.99.02",
            shared_size,
        };
        assert!(!malformed.is_valid());
        assert_eq!(malformed.mapping_length(4096), None);
    }
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
