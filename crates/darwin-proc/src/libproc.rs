// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::io;

use crate::{
    Pid,
    sysctl::{Plain, zeroed},
};

/// Calls `proc_pidinfo` with `flavor` and `argument` for one `T`.
///
/// `proc_pidinfo` returns the number of bytes that it wrote, and 0 or a
/// negative number with `errno` set when it fails. A short write leaves the
/// struct incomplete, so it is a failure. `pid_info` reports it as `ESRCH`
/// because callers treat that as an exited thread or process.
pub(crate) fn pid_info<T: Plain>(pid: Pid, flavor: libc::c_int, argument: u64) -> io::Result<T> {
    let mut value = zeroed::<T>();
    let written = pid_info_into(pid, flavor, argument, std::slice::from_mut(&mut value))?;

    if written < size_of::<T>() {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }

    Ok(value)
}

/// Calls `proc_pidinfo` with `flavor` and `argument` to fill `buffer`, and
/// returns the number of bytes that it wrote.
pub(crate) fn pid_info_into<T: Plain>(
    pid: Pid,
    flavor: libc::c_int,
    argument: u64,
    buffer: &mut [T],
) -> io::Result<usize> {
    let size = libc::c_int::try_from(std::mem::size_of_val(buffer)).map_err(io::Error::other)?;

    // SAFETY: `buffer` has `size` writable bytes, and `proc_pidinfo` writes at
    // most `size` bytes. `T: Plain`, so any bytes that it writes form valid
    // values.
    let written = unsafe {
        libc::proc_pidinfo(
            pid.as_raw(),
            flavor,
            argument,
            buffer.as_mut_ptr().cast::<libc::c_void>(),
            size,
        )
    };

    match usize::try_from(written) {
        Ok(written) if written > 0 => Ok(written),
        _ => Err(io::Error::last_os_error()),
    }
}
