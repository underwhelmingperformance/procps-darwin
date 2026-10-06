// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{ffi::OsStr, io, os::unix::ffi::OsStrExt, path::PathBuf};

use crate::{Call, Error, Pid};

/// `PROC_PIDPATHINFO_MAXSIZE` from `<sys/proc_info.h>`.
const PATH_MAX_SIZE: usize = 4 * libc::MAXPATHLEN as usize;

impl Pid {
    /// The path of the executable that the process is running.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use darwin_proc::Pid;
    ///
    /// assert_eq!(Pid::from(1).executable_path()?, Path::new("/sbin/launchd"));
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist,
    /// [`Error::Unsupported`] for `kernel_task` and zombies, which have no
    /// executable path, or another error if `proc_pidpath` fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn executable_path(self) -> Result<PathBuf, Error> {
        let mut buffer = vec![0_u8; PATH_MAX_SIZE];
        let size = u32::try_from(buffer.len()).map_err(|source| Error::Os {
            call: Call::ExecutablePath,
            source: io::Error::other(source),
        })?;

        // SAFETY: `buffer` has `size` writable bytes, and `proc_pidpath`
        // writes at most `size` bytes.
        let length = unsafe { libc::proc_pidpath(self.as_raw(), buffer.as_mut_ptr().cast(), size) };

        let length = usize::try_from(length).unwrap_or(0);

        if length == 0 {
            return Err(self.live_error(Call::ExecutablePath, io::Error::last_os_error()));
        }

        buffer.truncate(length);

        Ok(PathBuf::from(OsStr::from_bytes(&buffer)))
    }

    /// The session ID of the process.
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// assert_eq!(Pid::from(1).session()?, Pid::from(1));
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist,
    /// [`Error::Unsupported`] for `kernel_task` and zombies, or another error
    /// if `getsid` fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn session(self) -> Result<Pid, Error> {
        // `getsid(0)` returns the caller's session, not `kernel_task`'s.
        if self.as_raw() == 0 {
            return Err(Error::Unsupported {
                pid: self,
                call: Call::Session,
            });
        }

        // SAFETY: `getsid` takes a process ID and reads no memory of ours.
        let session = unsafe { libc::getsid(self.as_raw()) };

        if session < 0 {
            return Err(self.live_error(Call::Session, io::Error::last_os_error()));
        }

        Ok(Pid::from(session))
    }
}
