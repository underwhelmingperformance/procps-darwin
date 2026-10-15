// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::io;

use crate::{Call, Error, Pid};

impl Pid {
    /// Sends the signal numbered `signal` to this process, as `kill(2)` does,
    /// with the caller's permissions. Signal 0 checks that the process exists
    /// and that the caller may signal it.
    ///
    /// A `Pid` refers to one process, but `kill(2)` reads 0 as the caller's
    /// process group, -1 as every process that the caller may signal, and
    /// other negative pids as process groups. Darwin's `kernel_task` has pid
    /// 0. This function returns [`Error::Denied`], the error for a process
    /// that the caller may not signal, for every pid of 0 and below without
    /// calling `kill(2)`.
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// assert!(Pid::current().send_signal(0).is_ok());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist, and
    /// [`Error::Denied`] if the caller may not signal it, as for every pid of 0
    /// and below. Returns [`Error::Os`] for any other failure of `kill(2)`,
    /// such as `EINVAL` for a signal number that Darwin does not have.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn send_signal(self, signal: libc::c_int) -> Result<(), Error> {
        if self.as_raw() <= 0 {
            return Err(Error::Denied {
                pid: self,
                call: Call::Signal,
            });
        }

        // SAFETY: `kill` takes no pointers.
        let result = unsafe { libc::kill(self.as_raw(), signal) };

        if result == -1 {
            return Err(Error::for_process(
                Call::Signal,
                self,
                io::Error::last_os_error(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use rstest::rstest;

    use crate::{Call, Error, Pid};

    #[rstest]
    #[case::kernel_task(0)]
    #[case::all(-1)]
    #[case::group(-2)]
    fn a_pid_that_kill_reads_as_several_processes_is_denied(#[case] pid: i32) {
        let pid = Pid::from(pid);

        assert_matches!(
            pid.send_signal(0),
            Err(Error::Denied { pid: denied, call: Call::Signal }) if denied == pid
        );
    }

    #[test]
    fn a_process_that_does_not_exist_has_exited() {
        let pid = Pid::from(100_000);

        assert_matches!(pid.send_signal(0), Err(Error::Exited { pid: exited }) if exited == pid);
    }

    #[test]
    fn only_root_can_signal_launchd() {
        let launchd = Pid::from(1);
        let signalled = launchd.send_signal(0);

        if rustix::process::geteuid().is_root() {
            assert_matches!(signalled, Ok(()));
            return;
        }

        assert_matches!(
            signalled,
            Err(Error::Denied { pid, call: Call::Signal }) if pid == launchd
        );
    }

    #[test]
    fn this_process_can_be_signalled() {
        assert_matches!(Pid::current().send_signal(0), Ok(()));
    }
}
