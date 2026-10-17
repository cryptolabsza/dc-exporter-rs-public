// SPDX-License-Identifier: MIT
// Definitions adapted from NVIDIA open-gpu-kernel-modules 590.48.01,
// Copyright (c) 1999-2025 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// See docs/gpu-thermal-abi.md for pinned sources and the full permission notice.
//! Verified Linux x86-64 ABI only; these types never issue calls on other targets.

pub const DRIVER_VERSION: &str = "590.48.01";
pub const RUSD_SIZE: usize = 3144;
pub const MEMORY_TIMESTAMP_OFFSET: usize = 2328;
pub const MEMORY_VALUE_OFFSET: usize = 2336;
pub const THERMAL_POLL_MASK: u64 = 0x10;
pub const SEQ_START: u64 = 0xff00_0000_0000_0000;
pub const MAX_DEVICES: usize = 32;

/// Complete shared-data sizes from the separately pinned 590, 595 and 615 headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbiProfile {
    pub driver_version: &'static str,
    pub shared_size: usize,
}

pub fn profile_for_driver(driver_version: &str) -> Option<AbiProfile> {
    match driver_version {
        "590.48.01" => Some(AbiProfile {
            driver_version: DRIVER_VERSION,
            shared_size: RUSD_SIZE,
        }),
        "595.99.02" => Some(AbiProfile {
            driver_version: "595.99.02",
            shared_size: 3152,
        }),
        "615.71.09" => Some(AbiProfile {
            driver_version: "615.71.09",
            shared_size: 3184,
        }),
        _ => None,
    }
}

impl AbiProfile {
    pub fn is_valid(self) -> bool {
        profile_for_driver(self.driver_version) == Some(self)
    }

    pub fn mapping_length(self, page_size: usize) -> Option<usize> {
        if !self.is_valid() || !page_size.is_power_of_two() {
            return None;
        }
        self.shared_size
            .checked_add(page_size - 1)
            .map(|s| s & !(page_size - 1))
    }
}

/// Compatibility wrapper for the original exact 590 fixture.
pub fn mapping_length(page_size: usize) -> Option<usize> {
    profile_for_driver(DRIVER_VERSION)?.mapping_length(page_size)
}

/// Linux asm-generic/x86 ioctl encoding, deliberately independent of host libc macros.
pub const fn linux_ioctl(number: u32, size: usize) -> libc::c_ulong {
    ((3u64 << 30) | ((size as u64) << 16) | (b'F' as u64) << 8 | number as u64) as libc::c_ulong
}

#[repr(C)]
#[derive(Default)]
pub struct Alloc {
    pub root: u32,
    pub parent: u32,
    pub object: u32,
    pub class: u32,
    pub params: u64,
    pub params_size: u32,
    pub status: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct Control {
    pub client: u32,
    pub object: u32,
    pub command: u32,
    pub flags: u32,
    pub params: u64,
    pub params_size: u32,
    pub status: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct Free {
    pub root: u32,
    pub parent: u32,
    pub object: u32,
    pub status: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct MapMemory {
    pub client: u32,
    pub device: u32,
    pub memory: u32,
    pub offset: u64,
    pub length: u64,
    pub address: u64,
    pub status: u32,
    pub flags: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct MapMemoryFd {
    pub params: MapMemory,
    pub fd: i32,
}
#[repr(C)]
#[derive(Default)]
pub struct UnmapMemory {
    pub client: u32,
    pub device: u32,
    pub memory: u32,
    pub address: u64,
    pub status: u32,
    pub flags: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct DeviceAlloc {
    pub device_id: u32,
    pub client_share: u32,
    pub target_client: u32,
    pub target_device: u32,
    pub flags: u32,
    pub va_space_size: u64,
    pub va_start: u64,
    pub va_limit: u64,
    pub va_mode: u32,
}
#[repr(C)]
pub struct ClientAlloc {
    pub client: u32,
    pub process_id: u32,
    pub process_name: [u8; 100],
    pub os_pid_info: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PciInfo {
    pub domain: u32,
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CardInfo {
    pub valid: u8,
    pub pci: PciInfo,
    pub gpu_id: u32,
    pub interrupt_line: u16,
    pub reg_address: u64,
    pub reg_size: u64,
    pub fb_address: u64,
    pub fb_size: u64,
    pub minor: u32,
    pub dev_name: [u8; 10],
}
#[repr(C)]
pub struct AttachIds {
    pub ids: [u32; MAX_DEVICES],
    pub failed_id: u32,
}
#[repr(C)]
pub struct DetachIds {
    pub ids: [u32; MAX_DEVICES],
}
#[repr(C)]
pub struct UuidInfo {
    pub uuid: [u8; 256],
    pub flags: u32,
    pub gpu_id: u32,
    pub device_instance: u32,
    pub subdevice_instance: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct IdInfo {
    pub gpu_id: u32,
    pub gpu_flags: u32,
    pub device_instance: u32,
    pub subdevice_instance: u32,
    pub sli_status: u32,
    pub board_id: u32,
    pub gpu_instance: u32,
    pub numa_id: i32,
}

const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<Alloc>() == 32 && offset_of!(Alloc, params) == 16);
    assert!(size_of::<Control>() == 32 && size_of::<Free>() == 16);
    assert!(size_of::<MapMemory>() == 48 && size_of::<MapMemoryFd>() == 56);
    assert!(offset_of!(MapMemoryFd, fd) == 48 && size_of::<UnmapMemory>() == 32);
    assert!(size_of::<DeviceAlloc>() == 56 && size_of::<ClientAlloc>() == 120);
    assert!(size_of::<CardInfo>() == 72 && offset_of!(CardInfo, minor) == 56);
    assert!(size_of::<AttachIds>() == 132 && size_of::<UuidInfo>() == 272);
};
