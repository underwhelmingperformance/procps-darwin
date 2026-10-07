// SPDX-FileCopyrightText: 1991-2026 Free Software Foundation, Inc.
// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from glibc 2.44 sysdeps/gnu/errlist.h.

/// glibc's descriptions of the error numbers that both systems have. The table
/// lists only the numbers for which Darwin's description differs. Linux gives
/// `EOPNOTSUPP` and `ENOTSUP` one number, which glibc describes as "Operation
/// not supported", so the table gives Darwin's `EOPNOTSUPP` that description.
const GLIBC: [(libc::c_int, &str); 25] = [
    (libc::ENXIO, "No such device or address"),
    (libc::EBUSY, "Device or resource busy"),
    (libc::EXDEV, "Invalid cross-device link"),
    (libc::ENODEV, "No such device"),
    (libc::ERANGE, "Numerical result out of range"),
    (
        libc::EAFNOSUPPORT,
        "Address family not supported by protocol",
    ),
    (libc::EADDRNOTAVAIL, "Cannot assign requested address"),
    (libc::EISCONN, "Transport endpoint is already connected"),
    (libc::ENOTCONN, "Transport endpoint is not connected"),
    (
        libc::ESHUTDOWN,
        "Cannot send after transport endpoint shutdown",
    ),
    (libc::ETOOMANYREFS, "Too many references: cannot splice"),
    (libc::ETIMEDOUT, "Connection timed out"),
    (libc::EDQUOT, "Disk quota exceeded"),
    (libc::ESTALE, "Stale file handle"),
    (libc::EREMOTE, "Object is remote"),
    (libc::EOVERFLOW, "Value too large for defined data type"),
    (
        libc::EILSEQ,
        "Invalid or incomplete multibyte or wide character",
    ),
    (libc::EMULTIHOP, "Multihop attempted"),
    (libc::ENODATA, "No data available"),
    (libc::ENOLINK, "Link has been severed"),
    (libc::ENOSR, "Out of streams resources"),
    (libc::ENOSTR, "Device not a stream"),
    (libc::ETIME, "Timer expired"),
    (libc::EOPNOTSUPP, "Operation not supported"),
    (libc::EOWNERDEAD, "Owner died"),
];

/// glibc's `strerror` description of the error number `code`. procps-ng prints
/// this description. An error number that Linux lacks keeps Darwin's
/// description.
pub(crate) fn strerror(code: libc::c_int) -> String {
    GLIBC
        .iter()
        .find(|&&(known, _)| known == code)
        .map_or_else(|| darwin_proc::strerror(code), |&(_, text)| text.to_owned())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::strerror;

    #[rstest]
    #[case::same_text(libc::ENOENT, "No such file or directory")]
    #[case::glibc_text(libc::ENXIO, "No such device or address")]
    #[case::linux_has_one_number(libc::EOPNOTSUPP, "Operation not supported")]
    #[case::darwin_only(libc::EAUTH, "Authentication error")]
    fn a_description_is_glibc_s(#[case] code: libc::c_int, #[case] expected: &str) {
        assert_eq!(strerror(code), expected);
    }
}
