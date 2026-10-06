// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::{CStr, OsStr},
    os::{fd::RawFd, unix::ffi::OsStrExt},
    path::PathBuf,
};

use crate::{
    Call, Error, Pid,
    libproc::{pid_info, pid_info_into, pid_info_size},
    sysctl::zeroed,
};

impl Pid {
    /// The process's working directory. If the directory has been removed,
    /// the kernel still returns its old path, without the ` (deleted)` suffix
    /// that Linux adds.
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// assert_eq!(
    ///     Pid::current().working_directory()?,
    ///     std::env::current_dir()?.canonicalize()?
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist,
    /// [`Error::Unsupported`] for a zombie or if the kernel's path is not
    /// NUL-terminated, [`Error::Denied`] for another user's process or for
    /// `kernel_task`, or [`Error::Os`] if `proc_pidinfo` fails for another
    /// reason.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn working_directory(self) -> Result<PathBuf, Error> {
        let info = pid_info::<libc::proc_vnodepathinfo>(self, libc::PROC_PIDVNODEPATHINFO, 0)
            .map_err(|source| self.live_error(Call::WorkingDirectory, source))?;
        let bytes: Vec<u8> = info
            .pvi_cdir
            .vip_path
            .as_flattened()
            .iter()
            .map(|&byte| byte.cast_unsigned())
            .collect();

        let Ok(path) = CStr::from_bytes_until_nul(&bytes) else {
            return Err(Error::Unsupported {
                pid: self,
                call: Call::WorkingDirectory,
            });
        };

        Ok(PathBuf::from(OsStr::from_bytes(path.to_bytes())))
    }

    /// The process's open file descriptors, in ascending order.
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// assert!(Pid::current().file_descriptors()?.contains(&0));
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist,
    /// [`Error::Unsupported`] for a zombie, [`Error::Denied`] for another
    /// user's process or for `kernel_task`, or [`Error::Os`] if `proc_pidinfo`
    /// fails for another reason.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn file_descriptors(self) -> Result<Vec<RawFd>, Error> {
        let error = |source| self.live_error(Call::FileDescriptors, source);

        loop {
            let size = pid_info_size(self, libc::PROC_PIDLISTFDS, 0).map_err(error)?;
            let capacity = size / size_of::<libc::proc_fdinfo>();

            if capacity == 0 {
                return Ok(Vec::new());
            }

            let mut buffer = vec![zeroed::<libc::proc_fdinfo>(); capacity];
            let written =
                pid_info_into(self, libc::PROC_PIDLISTFDS, 0, &mut buffer).map_err(error)?;
            let count = written / size_of::<libc::proc_fdinfo>();

            // A full buffer can mean that the process opened more descriptors
            // after the kernel reported the size, so the list may be
            // incomplete.
            if count < capacity {
                return Ok(buffer[..count].iter().map(|info| info.proc_fd).collect());
            }
        }
    }
}
