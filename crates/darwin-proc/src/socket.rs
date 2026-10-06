// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::CStr,
    io,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
};

use crate::{Call, Error, Gid, Pid, Uid, ffi};

/// The size of a `pid_t`, which `LOCAL_PEERPID` returns.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a pid_t has 4 bytes, which fits in a socklen_t"
)]
const PID_LENGTH: libc::socklen_t = size_of::<libc::pid_t>() as libc::socklen_t;

/// The effective user and group IDs of the process at the other end of a
/// connected Unix-domain socket, from `getpeereid`. They remain readable after
/// the peer closes the connection.
///
/// ```
/// use std::os::unix::net::UnixStream;
///
/// use darwin_proc::{PeerCredentials, Pid};
///
/// let (ours, _theirs) = UnixStream::pair()?;
///
/// assert_eq!(
///     PeerCredentials::of(&ours)?.uid,
///     Pid::current().info()?.credentials.euid
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerCredentials {
    /// The peer's effective user ID when it connected, or when it called
    /// `listen` if it is the listening end.
    pub uid: Uid,
    /// The peer's effective group ID when it connected, or when it called
    /// `listen` if it is the listening end.
    pub gid: Gid,
}

impl PeerCredentials {
    /// Reads the credentials of the peer of `socket`.
    ///
    /// ```
    /// use std::os::unix::net::UnixStream;
    ///
    /// use darwin_proc::PeerCredentials;
    ///
    /// let (ours, theirs) = UnixStream::pair()?;
    ///
    /// assert_eq!(PeerCredentials::of(&ours)?, PeerCredentials::of(&theirs)?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Os`] if `socket` is not a Unix-domain socket that has
    /// been connected.
    #[tracing::instrument(level = "debug", skip_all, err(level = "debug"))]
    pub fn of(socket: impl AsFd) -> Result<Self, Error> {
        let fd = socket.as_fd().as_raw_fd();
        let mut uid = 0;
        let mut gid = 0;

        // SAFETY: `socket` keeps `fd` open for the call, and `uid` and `gid`
        // are valid for writes.
        if unsafe { libc::getpeereid(fd, &raw mut uid, &raw mut gid) } != 0 {
            return Err(Error::Os {
                call: Call::PeerCredentials,
                source: io::Error::last_os_error(),
            });
        }

        Ok(Self {
            uid: Uid::from(uid),
            gid: Gid::from(gid),
        })
    }
}

/// The process at the other end of a connected Unix-domain socket.
///
/// ```
/// use std::os::unix::net::UnixStream;
///
/// use darwin_proc::{Peer, Pid};
///
/// let (ours, _theirs) = UnixStream::pair()?;
///
/// assert_eq!(Peer::of(&ours)?.pid, Pid::current());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Peer {
    /// The effective user ID when the peer connected, or called `listen` if it
    /// is the listening end, from `getpeereid`.
    pub uid: Uid,
    /// The effective group ID when the peer connected, or called `listen` if it
    /// is the listening end, from `getpeereid`.
    pub gid: Gid,
    /// The process ID, from `LOCAL_PEERPID`. The kernel updates it whenever a
    /// process uses the peer's end of the connection, so it identifies the
    /// last process that used that end, which can be a child of the process
    /// that connected.
    pub pid: Pid,
}

impl Peer {
    /// Reads the peer of `socket`.
    ///
    /// ```
    /// use std::os::unix::net::UnixStream;
    ///
    /// use darwin_proc::Peer;
    ///
    /// let (ours, theirs) = UnixStream::pair()?;
    ///
    /// assert_eq!(Peer::of(&ours)?, Peer::of(&theirs)?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Os`] if `socket` is not a connected Unix-domain socket,
    /// including after the peer has closed it.
    #[tracing::instrument(level = "debug", skip_all, err(level = "debug"))]
    pub fn of(socket: impl AsFd) -> Result<Self, Error> {
        let credentials = PeerCredentials::of(&socket)?;
        let fd = socket.as_fd().as_raw_fd();
        let mut pid: libc::pid_t = 0;
        let mut length = PID_LENGTH;

        // SAFETY: `socket` keeps `fd` open for the call, `pid` has `length`
        // writable bytes, and `length` is valid for writes.
        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&raw mut pid).cast(),
                &raw mut length,
            )
        };

        if result != 0 {
            return Err(Error::Os {
                call: Call::PeerProcess,
                source: io::Error::last_os_error(),
            });
        }

        if length != PID_LENGTH {
            return Err(Error::Os {
                call: Call::PeerProcess,
                source: io::ErrorKind::InvalidData.into(),
            });
        }

        Ok(Self {
            uid: credentials.uid,
            gid: credentials.gid,
            pid: Pid::from(pid),
        })
    }
}

/// The sockets that launchd creates for a job from the `Sockets` entry in its
/// property list.
///
/// ```
/// use darwin_proc::LaunchdSockets;
///
/// assert!(LaunchdSockets::activate(c"Listeners").is_err());
/// ```
#[derive(Clone, Copy, Debug)]
pub struct LaunchdSockets;

impl LaunchdSockets {
    /// Takes the sockets that launchd created for the entry `name` in the
    /// `Sockets` dictionary of this process's job.
    ///
    /// ```
    /// use darwin_proc::LaunchdSockets;
    ///
    /// // launchd did not start the doctest.
    /// assert!(LaunchdSockets::activate(c"Listeners").is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Os`] if launchd did not start this process, if the job
    /// has no `Sockets` entry called `name`, or if this process has already
    /// taken the sockets.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn activate(name: &CStr) -> Result<Vec<OwnedFd>, Error> {
        let mut fds: *mut libc::c_int = std::ptr::null_mut();
        let mut count: libc::size_t = 0;

        // SAFETY: `name` is NUL-terminated, and `fds` and `count` are valid
        // for writes.
        let code =
            unsafe { ffi::launch_activate_socket(name.as_ptr(), &raw mut fds, &raw mut count) };

        if code != 0 {
            return Err(Error::Os {
                call: Call::LaunchdSockets,
                source: io::Error::from_raw_os_error(code),
            });
        }

        if fds.is_null() {
            return Ok(Vec::new());
        }

        // SAFETY: on success, `fds` points to `count` descriptors, and it is
        // not null.
        let raw = unsafe { std::slice::from_raw_parts(fds, count) };
        let sockets = raw
            .iter()
            // SAFETY: launchd gives the descriptors to this process, which
            // owns each of them from here on.
            .map(|&fd| unsafe { OwnedFd::from_raw_fd(fd) })
            .collect();

        // SAFETY: launchd allocated the array with `malloc`, and nothing reads
        // it after this call.
        unsafe { libc::free(fds.cast()) };

        Ok(sockets)
    }
}
