// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{ffi::CStr, io};

use crate::ffi::{KinfoProc, RusageInfoV6};

/// A type that the kernel may fill with any bytes.
///
/// # Safety
///
/// Every bit pattern must be a valid value of the type.
pub(crate) unsafe trait Plain {}

// SAFETY: every bit pattern is a valid `u8`.
unsafe impl Plain for u8 {}

// SAFETY: every bit pattern is a valid `c_int`.
unsafe impl Plain for libc::c_int {}

// SAFETY: every bit pattern is a valid `u64`.
unsafe impl Plain for u64 {}

// SAFETY: `KinfoProc` contains only integers, arrays of integers and raw
// pointers, for which every bit pattern is valid.
unsafe impl Plain for KinfoProc {}

// SAFETY: `RusageInfoV6` contains only integers and arrays of integers.
unsafe impl Plain for RusageInfoV6 {}

// SAFETY: `proc_taskinfo` contains only integers.
unsafe impl Plain for libc::proc_taskinfo {}

// SAFETY: `proc_threadinfo` contains only integers and an array of `c_char`.
unsafe impl Plain for libc::proc_threadinfo {}

/// A `T` with every byte zero, for the kernel to fill.
pub(crate) fn zeroed<T: Plain>() -> T {
    // SAFETY: `T: Plain`, so all-zero bytes are a valid value.
    unsafe { std::mem::zeroed() }
}

/// Calls `sysctl` for `mib`. With `buffer`, the kernel fills it and sets
/// `size` to the number of bytes that it wrote. Without a buffer, the kernel
/// sets `size` to the number of bytes that it would write.
pub(crate) fn sysctl<T: Plain>(
    mib: &mut [libc::c_int],
    buffer: Option<&mut [T]>,
    size: &mut usize,
) -> io::Result<()> {
    let length = libc::c_uint::try_from(mib.len()).map_err(io::Error::other)?;
    let pointer = buffer.map_or(std::ptr::null_mut(), |buffer| {
        *size = (*size).min(std::mem::size_of_val(buffer));
        buffer.as_mut_ptr().cast::<libc::c_void>()
    });

    // SAFETY: `mib` is valid for `length` integers. `pointer` is either null,
    // which asks only for the size, or points to a buffer of at least `*size`
    // bytes, because `*size` is clamped to the buffer's length above. `T` is
    // `Plain`, so any bytes that the kernel writes form valid values.
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            length,
            pointer,
            size,
            std::ptr::null_mut(),
            0,
        )
    };

    if result != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

/// Reads the sysctl `name`, whose value is a single `T`.
pub(crate) fn sysctl_value<T: Plain + Default>(name: &CStr) -> io::Result<T> {
    let mut value = T::default();
    let mut size = size_of::<T>();

    // SAFETY: `name` is a NUL-terminated string, and `value` has `size`
    // writable bytes. `T: Plain`, so any bytes that the kernel writes form a
    // valid value.
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&raw mut value).cast::<libc::c_void>(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };

    if result != 0 {
        return Err(io::Error::last_os_error());
    }

    if size != size_of::<T>() {
        return Err(io::Error::other(format!(
            "sysctl {} returned {size} bytes, not {}",
            name.to_string_lossy(),
            size_of::<T>()
        )));
    }

    Ok(value)
}
