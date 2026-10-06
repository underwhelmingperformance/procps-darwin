// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::{CStr, CString, OsStr, OsString},
    io,
    mem::MaybeUninit,
    os::unix::ffi::OsStrExt,
};

/// A user ID.
///
/// ```
/// use darwin_proc::Uid;
///
/// assert_eq!(
///     (Uid::from(501).as_raw(), Uid::from(501).to_string()),
///     (501, "501".to_owned())
/// );
/// ```
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    derive_more::Display,
    derive_more::From,
    derive_more::Into,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    expect(
        clippy::unsafe_derive_deserialize,
        reason = "the user database lookups accept any user ID"
    )
)]
pub struct Uid(libc::uid_t);

/// A group ID.
///
/// ```
/// use darwin_proc::Gid;
///
/// assert_eq!(
///     (Gid::from(20).as_raw(), Gid::from(20).to_string()),
///     (20, "20".to_owned())
/// );
/// ```
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    derive_more::Display,
    derive_more::From,
    derive_more::Into,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    expect(
        clippy::unsafe_derive_deserialize,
        reason = "the group database lookups accept any group ID"
    )
)]
pub struct Gid(libc::gid_t);

/// The size of the first buffer for the strings of a user or group entry.
const INITIAL_BUFFER_SIZE: usize = 1024;

/// The largest buffer to try before giving up on a user or group entry.
const MAXIMUM_BUFFER_SIZE: usize = 1024 * 1024;

/// Looks up a user or group entry with `call`, one of the `getpw*_r` and
/// `getgr*_r` functions, and returns what `read` takes from the entry, or
/// `None` if the database has no such entry.
///
/// `call` receives room for one entry, a buffer for the entry's strings with
/// its length, and a pointer to writable storage for the result. The functions
/// return `ERANGE` when the buffer is too small, and `lookup` then doubles the
/// buffer, up to [`MAXIMUM_BUFFER_SIZE`]. `lookup` calls `read` only with an
/// entry for which `call` returned 0.
///
/// # Safety
///
/// When `call` returns 0 and leaves the result non-null, it must have filled
/// the entry so that its strings are in the buffer.
unsafe fn lookup<E, T>(
    call: impl Fn(*mut E, *mut libc::c_char, usize, *mut *mut E) -> libc::c_int,
    read: impl Fn(&E) -> T,
) -> io::Result<Option<T>> {
    let mut size = INITIAL_BUFFER_SIZE;

    loop {
        let mut buffer = vec![0; size];
        let mut entry = MaybeUninit::<E>::uninit();
        let mut result: *mut E = std::ptr::null_mut();
        let status = call(
            entry.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &raw mut result,
        );

        match status {
            0 if result.is_null() => return Ok(None),
            // SAFETY: `call` returned 0 and left `result` non-null, so the
            // caller guarantees that `call` filled `entry`. The entry's strings
            // are in `buffer`, which lives until the end of this iteration.
            0 => return Ok(Some(read(unsafe { entry.assume_init_ref() }))),
            libc::ERANGE if size < MAXIMUM_BUFFER_SIZE => size *= 2,
            code => return Err(io::Error::from_raw_os_error(code)),
        }
    }
}

/// Copies the NUL-terminated string at `name`.
///
/// # Safety
///
/// `name` must point to a NUL-terminated string.
unsafe fn copy_name(name: *const libc::c_char) -> OsString {
    // SAFETY: the caller guarantees that `name` points to a NUL-terminated
    // string.
    let name = unsafe { CStr::from_ptr(name) };

    OsStr::from_bytes(name.to_bytes()).to_owned()
}

impl Uid {
    /// The user ID as the C type `uid_t`.
    ///
    /// ```
    /// use darwin_proc::Uid;
    ///
    /// assert_eq!(Uid::from(0).as_raw(), 0);
    /// ```
    #[must_use]
    pub const fn as_raw(self) -> libc::uid_t {
        self.0
    }

    /// The user's name from the user database, or `None` if the database has
    /// no user with this ID.
    ///
    /// ```
    /// use darwin_proc::Uid;
    ///
    /// assert_eq!(Uid::from(0).name()?, Some("root".into()));
    /// # Ok::<(), std::io::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns the error from `getpwuid_r` if the lookup fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn name(self) -> io::Result<Option<OsString>> {
        // SAFETY: `lookup` passes room for one `passwd`, a buffer of `length`
        // writable bytes and writable storage for the result. When `getpwuid_r`
        // returns 0 with a non-null result, it has filled the entry and the
        // entry's strings are in the buffer, as `lookup` requires. `pw_name` in
        // that entry is NUL-terminated.
        unsafe {
            lookup(
                |entry, buffer, length, result| {
                    libc::getpwuid_r(self.0, entry, buffer, length, result)
                },
                |entry: &libc::passwd| copy_name(entry.pw_name),
            )
        }
    }

    /// The ID of the user with this name in the user database, or `None` if
    /// the database has no such user.
    ///
    /// ```
    /// use darwin_proc::Uid;
    ///
    /// assert_eq!(Uid::from_name("root".as_ref())?, Some(Uid::from(0)));
    /// # Ok::<(), std::io::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns the error from `getpwnam_r` if the lookup fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn from_name(name: &OsStr) -> io::Result<Option<Self>> {
        // A name with a NUL byte cannot be in the database.
        let Ok(name) = CString::new(name.as_bytes()) else {
            return Ok(None);
        };

        // SAFETY: `name` is NUL-terminated, and `lookup` passes room for one
        // `passwd`, a buffer of `length` writable bytes and writable storage
        // for the result. When `getpwnam_r` returns 0 with a non-null result,
        // it has filled the entry and the entry's strings are in the buffer, as
        // `lookup` requires.
        unsafe {
            lookup(
                |entry, buffer, length, result| {
                    libc::getpwnam_r(name.as_ptr(), entry, buffer, length, result)
                },
                |entry: &libc::passwd| Self(entry.pw_uid),
            )
        }
    }
}

impl Gid {
    /// The group ID as the C type `gid_t`.
    ///
    /// ```
    /// use darwin_proc::Gid;
    ///
    /// assert_eq!(Gid::from(0).as_raw(), 0);
    /// ```
    #[must_use]
    pub const fn as_raw(self) -> libc::gid_t {
        self.0
    }

    /// The group's name from the group database, or `None` if the database
    /// has no group with this ID.
    ///
    /// ```
    /// use darwin_proc::Gid;
    ///
    /// assert_eq!(Gid::from(0).name()?, Some("wheel".into()));
    /// # Ok::<(), std::io::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns the error from `getgrgid_r` if the lookup fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn name(self) -> io::Result<Option<OsString>> {
        // SAFETY: `lookup` passes room for one `group`, a buffer of `length`
        // writable bytes and writable storage for the result. When `getgrgid_r`
        // returns 0 with a non-null result, it has filled the entry and the
        // entry's strings are in the buffer, as `lookup` requires. `gr_name` in
        // that entry is NUL-terminated.
        unsafe {
            lookup(
                |entry, buffer, length, result| {
                    libc::getgrgid_r(self.0, entry, buffer, length, result)
                },
                |entry: &libc::group| copy_name(entry.gr_name),
            )
        }
    }

    /// The ID of the group with this name in the group database, or `None` if
    /// the database has no such group.
    ///
    /// ```
    /// use darwin_proc::Gid;
    ///
    /// assert_eq!(Gid::from_name("wheel".as_ref())?, Some(Gid::from(0)));
    /// # Ok::<(), std::io::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns the error from `getgrnam_r` if the lookup fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn from_name(name: &OsStr) -> io::Result<Option<Self>> {
        // A name with a NUL byte cannot be in the database.
        let Ok(name) = CString::new(name.as_bytes()) else {
            return Ok(None);
        };

        // SAFETY: `name` is NUL-terminated, and `lookup` passes room for one
        // `group`, a buffer of `length` writable bytes and writable storage for
        // the result. When `getgrnam_r` returns 0 with a non-null result, it
        // has filled the entry and the entry's strings are in the buffer, as
        // `lookup` requires.
        unsafe {
            lookup(
                |entry, buffer, length, result| {
                    libc::getgrnam_r(name.as_ptr(), entry, buffer, length, result)
                },
                |entry: &libc::group| Self(entry.gr_gid),
            )
        }
    }
}
