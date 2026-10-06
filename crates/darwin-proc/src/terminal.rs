// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::{CStr, OsStr, OsString},
    os::unix::ffi::OsStrExt,
};

use crate::{Terminal, ffi};

/// The size of the buffer for a device name. XNU's devfs limits a name to
/// `DEVMAXNAMESIZE` (32) bytes and a path within `/dev` to `DEVMAXPATHSIZE`
/// (128) bytes, so every name fits.
const NAME_SIZE: u16 = 256;

impl Terminal {
    /// The terminal's name in `/dev`, such as `ttys001`, or `None` if `/dev`
    /// has no character device with this device number.
    ///
    /// ```
    /// use darwin_proc::{Pid, Terminal};
    ///
    /// let unknown = Terminal {
    ///     device: 0x7fff_ffff,
    ///     foreground_group: Pid::from(1),
    /// };
    ///
    /// assert_eq!(unknown.name(), None);
    /// ```
    #[must_use]
    #[tracing::instrument(level = "debug")]
    pub fn name(&self) -> Option<OsString> {
        let mut buffer = vec![0 as libc::c_char; usize::from(NAME_SIZE)];

        // SAFETY: `buffer` has `NAME_SIZE` writable bytes, and `devname_r`
        // writes at most that many, including the terminating NUL.
        let name = unsafe {
            ffi::devname_r(
                self.device,
                libc::S_IFCHR,
                buffer.as_mut_ptr(),
                libc::c_int::from(NAME_SIZE),
            )
        };

        if name.is_null() {
            return None;
        }

        // SAFETY: `devname_r` succeeded, so `name` points to a NUL-terminated
        // string in `buffer`.
        let name = unsafe { CStr::from_ptr(name) };

        Some(OsStr::from_bytes(name.to_bytes()).to_owned())
    }
}
