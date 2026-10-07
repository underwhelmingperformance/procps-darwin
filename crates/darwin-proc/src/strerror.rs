// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::CStr;

/// The C library's message for the error number `code`, as `strerror`
/// returns it. Unlike the `Display` output of [`std::io::Error`], it has no
/// `(os error N)` suffix, so the tools can print messages in the form that C
/// programs use.
///
/// ```
/// assert_eq!(
///     darwin_proc::strerror(libc::ENOENT),
///     "No such file or directory"
/// );
/// ```
#[must_use]
pub fn strerror(code: libc::c_int) -> String {
    let mut buffer = [0_u8; 256];

    // SAFETY: `buffer` has `buffer.len()` writable bytes, and `strerror_r`
    // writes a NUL-terminated message of at most that many bytes. For an
    // unknown code it writes "Unknown error: <code>" and returns `EINVAL`.
    // The message is still NUL-terminated.
    unsafe {
        libc::strerror_r(code, buffer.as_mut_ptr().cast(), buffer.len());
    }

    CStr::from_bytes_until_nul(&buffer)
        .map(|message| message.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::strerror;

    #[test]
    fn an_unknown_error_has_the_c_library_s_message() {
        assert_eq!(strerror(12345), "Unknown error: 12345");
    }
}
