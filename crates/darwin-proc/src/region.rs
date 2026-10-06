// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{io, num::NonZeroU32, sync::OnceLock};

use crate::{
    Call, Error, Pid,
    ffi::{self, ProcRegionInfo},
    libproc::pid_info,
    sysctl::sysctl_value,
};

/// One region of a process's virtual memory, from `proc_pidinfo` with
/// `PROC_PIDREGIONINFO`.
///
/// The sizes are in bytes. The kernel reports most of them in pages. It counts
/// `private_resident` and `shared_resident` in its own 16 KiB pages. It counts
/// the other sizes in pages of the smaller of the caller's and the target's
/// page sizes. This crate converts every count with the caller's page size,
/// which is 16 KiB for an arm64 process such as the tools. On Apple silicon,
/// only an `x86_64` program running under Rosetta can have 4 KiB pages. The
/// `resident`, `shared_now_private`, `swapped_out` and `dirtied` sizes of such
/// a program's regions are then four times too large.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    /// The first address of the region.
    pub address: u64,
    /// The size of the region.
    pub size: u64,
    /// What the process may currently do with the region.
    pub protection: Protection,
    /// The highest protection that the process may set on the region.
    pub max_protection: Protection,
    /// How the region's pages are shared with other processes.
    pub share_mode: ShareMode,
    /// The tag that identifies what the memory is for, such as a malloc zone
    /// or a stack. The values are the `VM_MEMORY_*` constants in
    /// `<mach/vm_statistics.h>`.
    pub user_tag: u32,
    /// The part of the region that is resident in memory.
    pub resident: u64,
    /// The resident part that only this process uses.
    pub private_resident: u64,
    /// The resident part that other processes can also use.
    pub shared_resident: u64,
    /// The part that this process has copied from shared pages by writing to
    /// them.
    pub shared_now_private: u64,
    /// The part that is in swap.
    pub swapped_out: u64,
    /// The part that this process has written to.
    pub dirtied: u64,
    /// The number of references to the region's memory object.
    pub reference_count: u32,
    /// An identifier for the region's memory object, or `None` for a region
    /// without one, such as a submap. Regions that map the same object have the
    /// same identifier, whether they are in this process or in another. The
    /// kernel derives it from a hash of the object's address, truncated to
    /// 32 bits, so two different objects can have the same identifier.
    pub object_id: Option<NonZeroU32>,
    /// `PROC_REGION_SUBMAP`: the region maps a submap, such as the shared
    /// cache of system libraries.
    pub is_submap: bool,
    /// `PROC_REGION_SHARED`: the region's memory object is shared with
    /// another map.
    pub is_shared: bool,
}

/// The `VM_PROT_*` permissions of a region.
///
/// ```
/// use darwin_proc::Protection;
///
/// let protection = Protection::from(0x5);
///
/// assert_eq!(
///     (
///         protection.is_readable(),
///         protection.is_writable(),
///         protection.is_executable()
///     ),
///     (true, false, true)
/// );
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, derive_more::From)]
pub struct Protection(u32);

impl Protection {
    /// Whether the process may read the region (`VM_PROT_READ`).
    ///
    /// ```
    /// use darwin_proc::Protection;
    ///
    /// assert!(Protection::from(0x1).is_readable());
    /// ```
    #[must_use]
    pub const fn is_readable(self) -> bool {
        self.0 & ffi::VM_PROT_READ != 0
    }

    /// Whether the process may write to the region (`VM_PROT_WRITE`).
    ///
    /// ```
    /// use darwin_proc::Protection;
    ///
    /// assert!(Protection::from(0x2).is_writable());
    /// ```
    #[must_use]
    pub const fn is_writable(self) -> bool {
        self.0 & ffi::VM_PROT_WRITE != 0
    }

    /// Whether the process may run code from the region (`VM_PROT_EXECUTE`).
    ///
    /// ```
    /// use darwin_proc::Protection;
    ///
    /// assert!(Protection::from(0x4).is_executable());
    /// ```
    #[must_use]
    pub const fn is_executable(self) -> bool {
        self.0 & ffi::VM_PROT_EXECUTE != 0
    }
}

/// How a region's pages are shared, from the `SM_*` constants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShareMode {
    /// `SM_COW`: shared until the process writes to a page, which the kernel
    /// then copies.
    CopyOnWrite,
    /// `SM_PRIVATE`: used only by this process.
    Private,
    /// `SM_EMPTY`: no pages.
    Empty,
    /// `SM_SHARED`: shared with other processes.
    Shared,
    /// `SM_TRUESHARED`: shared, and writes are visible to the other processes.
    TrueShared,
    /// `SM_PRIVATE_ALIASED`: private, but mapped more than once in this
    /// process.
    PrivateAliased,
    /// `SM_SHARED_ALIASED`: shared, and mapped more than once in this process.
    SharedAliased,
    /// `SM_LARGE_PAGE`: backed by large pages.
    LargePage,
    /// A value that this crate does not recognise.
    Other(u32),
}

impl From<u32> for ShareMode {
    /// Converts an `SM_*` value from `<sys/proc_info.h>`.
    ///
    /// ```
    /// use darwin_proc::ShareMode;
    ///
    /// assert_eq!(
    ///     (ShareMode::from(2), ShareMode::from(9)),
    ///     (ShareMode::Private, ShareMode::Other(9))
    /// );
    /// ```
    fn from(mode: u32) -> Self {
        match mode {
            ffi::SM_COW => Self::CopyOnWrite,
            ffi::SM_PRIVATE => Self::Private,
            ffi::SM_EMPTY => Self::Empty,
            ffi::SM_SHARED => Self::Shared,
            ffi::SM_TRUESHARED => Self::TrueShared,
            ffi::SM_PRIVATE_ALIASED => Self::PrivateAliased,
            ffi::SM_SHARED_ALIASED => Self::SharedAliased,
            ffi::SM_LARGE_PAGE => Self::LargePage,
            other => Self::Other(other),
        }
    }
}

impl Region {
    /// Decodes `raw`, converting every page count to bytes with `page_size`.
    pub(crate) fn decode(raw: &ProcRegionInfo, page_size: u64) -> Self {
        let bytes = |pages: u32| u64::from(pages) * page_size;

        Self {
            address: raw.pri_address,
            size: raw.pri_size,
            protection: Protection::from(raw.pri_protection),
            max_protection: Protection::from(raw.pri_max_protection),
            share_mode: ShareMode::from(raw.pri_share_mode),
            user_tag: raw.pri_user_tag,
            resident: bytes(raw.pri_pages_resident),
            private_resident: bytes(raw.pri_private_pages_resident),
            shared_resident: bytes(raw.pri_shared_pages_resident),
            shared_now_private: bytes(raw.pri_pages_shared_now_private),
            swapped_out: bytes(raw.pri_pages_swapped_out),
            dirtied: bytes(raw.pri_pages_dirtied),
            reference_count: raw.pri_ref_count,
            object_id: NonZeroU32::new(raw.pri_obj_id),
            is_submap: raw.pri_flags & ffi::PROC_REGION_SUBMAP != 0,
            is_shared: raw.pri_flags & ffi::PROC_REGION_SHARED != 0,
        }
    }
}

/// The caller's page size in bytes. It does not change, so the first
/// successful read is cached.
fn page_size() -> io::Result<u64> {
    static PAGE_SIZE: OnceLock<u64> = OnceLock::new();

    if let Some(&size) = PAGE_SIZE.get() {
        return Ok(size);
    }

    let size = sysctl_value::<u64>(c"hw.pagesize")?;

    if size == 0 {
        return Err(io::Error::other("sysctl hw.pagesize returned 0"));
    }

    Ok(*PAGE_SIZE.get_or_init(|| size))
}

impl Pid {
    /// Reads every region of the process's virtual memory, in address order.
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// assert!(!Pid::current().regions()?.is_empty());
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist,
    /// [`Error::Unsupported`] for a zombie, [`Error::Denied`] for another
    /// user's process or for `kernel_task`, or [`Error::Os`] if `proc_pidinfo`
    /// fails for another reason or the `hw.pagesize` sysctl fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn regions(self) -> Result<Vec<Region>, Error> {
        let page_size = page_size().map_err(|source| Error::Os {
            call: Call::Regions,
            source,
        })?;
        let mut regions = Vec::new();
        let mut address = 0;

        loop {
            match pid_info::<ProcRegionInfo>(self, ffi::PROC_PIDREGIONINFO, address) {
                Ok(raw) => {
                    address = raw.pri_address.saturating_add(raw.pri_size);
                    regions.push(Region::decode(&raw, page_size));
                }
                // The kernel returns `EINVAL` when no region starts at or
                // above `address`, which ends the walk.
                Err(error) if error.raw_os_error() == Some(libc::EINVAL) => {
                    return Ok(regions);
                }
                Err(source) => return Err(self.live_error(Call::Regions, source)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Protection, Region, ShareMode};
    use crate::{ffi::ProcRegionInfo, sysctl::zeroed};

    #[test]
    fn every_field_is_decoded() {
        let mut raw = zeroed::<ProcRegionInfo>();
        raw.pri_address = 0x1_0000_0000;
        raw.pri_size = 0x8000;
        raw.pri_protection = 0x3;
        raw.pri_max_protection = 0x7;
        raw.pri_share_mode = 1;
        raw.pri_user_tag = 30;
        raw.pri_pages_resident = 2;
        raw.pri_private_pages_resident = 1;
        raw.pri_shared_pages_resident = 3;
        raw.pri_pages_shared_now_private = 4;
        raw.pri_pages_swapped_out = 5;
        raw.pri_pages_dirtied = 6;
        raw.pri_ref_count = 7;
        raw.pri_obj_id = 8;
        raw.pri_flags = 0x3;

        assert_eq!(
            Region::decode(&raw, 16_384),
            Region {
                address: 0x1_0000_0000,
                size: 0x8000,
                protection: Protection::from(0x3),
                max_protection: Protection::from(0x7),
                share_mode: ShareMode::CopyOnWrite,
                user_tag: 30,
                resident: 32_768,
                private_resident: 16_384,
                shared_resident: 49_152,
                shared_now_private: 65_536,
                swapped_out: 81_920,
                dirtied: 98_304,
                reference_count: 7,
                object_id: NonZeroU32::new(8),
                is_submap: true,
                is_shared: true,
            }
        );
    }

    #[test]
    fn a_region_without_an_object_has_no_object_id() {
        let mut raw = zeroed::<ProcRegionInfo>();
        raw.pri_share_mode = 3;

        assert_eq!(
            Region::decode(&raw, 16_384),
            Region {
                address: 0,
                size: 0,
                protection: Protection::default(),
                max_protection: Protection::default(),
                share_mode: ShareMode::Empty,
                user_tag: 0,
                resident: 0,
                private_resident: 0,
                shared_resident: 0,
                shared_now_private: 0,
                swapped_out: 0,
                dirtied: 0,
                reference_count: 0,
                object_id: None,
                is_submap: false,
                is_shared: false,
            }
        );
    }

    #[rstest]
    #[case::copy_on_write(1, ShareMode::CopyOnWrite)]
    #[case::private(2, ShareMode::Private)]
    #[case::empty(3, ShareMode::Empty)]
    #[case::shared(4, ShareMode::Shared)]
    #[case::true_shared(5, ShareMode::TrueShared)]
    #[case::private_aliased(6, ShareMode::PrivateAliased)]
    #[case::shared_aliased(7, ShareMode::SharedAliased)]
    #[case::large_page(8, ShareMode::LargePage)]
    #[case::unknown(9, ShareMode::Other(9))]
    fn a_share_mode_is_converted(#[case] raw: u32, #[case] expected: ShareMode) {
        assert_eq!(ShareMode::from(raw), expected);
    }
}
