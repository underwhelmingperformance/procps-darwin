// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsString;

use darwin_proc::ThreadInfo;

use crate::{Charset, Field, Process};

/// A Linux task: a process, or one of its threads. Linux calls each thread a
/// task, and `pgrep -w` and `ps -L` list tasks.
#[derive(Clone, Copy, Debug)]
pub struct LinuxTask<'a> {
    process: &'a Process,
    kind: Kind<'a>,
}

/// What a [`LinuxTask`] is.
#[derive(Clone, Copy, Debug)]
enum Kind<'a> {
    /// The whole process.
    Process,
    /// One of the process's threads.
    Thread {
        thread: &'a ThreadInfo,
        /// Whether the thread is the process's main thread, which has the
        /// process's name on Linux.
        main: bool,
    },
}

impl Process {
    /// The process as one task, as `pgrep` without `-w` sees it.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let pid = u64::from(info.pid.as_raw().unsigned_abs());
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_task().id(), pid);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub const fn linux_task(&self) -> LinuxTask<'_> {
        LinuxTask {
            process: self,
            kind: Kind::Process,
        }
    }

    /// The process's threads as Linux tasks, in ascending order of thread ID.
    /// When the threads are unavailable or empty, the process is one task whose
    /// ID is the pid, as a single-threaded process is on Linux.
    ///
    /// Darwin does not mark a process's main thread. Darwin assigns thread IDs
    /// in increasing order, so the main thread, the first thread of the
    /// process, has the lowest ID. That thread is treated as the main thread
    /// and has the process's name, as on Linux. Once the main thread has
    /// exited, the thread with the next lowest ID is treated as the main
    /// thread, whereas Linux keeps the main thread as a zombie.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let mut process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    /// process.threads = Some(Field::Available(Pid::current().threads()?));
    ///
    /// assert!(!process.linux_tasks().is_empty());
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn linux_tasks(&self) -> Vec<LinuxTask<'_>> {
        let threads = self.threads.as_ref().and_then(Field::available);
        let Some(threads) = threads.filter(|threads| !threads.is_empty()) else {
            return vec![self.linux_task()];
        };

        let mut threads: Vec<&ThreadInfo> = threads.iter().collect();
        threads.sort_by_key(|thread| thread.id);

        threads
            .into_iter()
            .enumerate()
            .map(|(index, thread)| LinuxTask {
                process: self,
                kind: Kind::Thread {
                    thread,
                    main: index == 0,
                },
            })
            .collect()
    }
}

impl<'a> LinuxTask<'a> {
    /// The process, or the thread's process.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_task().process().info.pid, Pid::current());
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub const fn process(&self) -> &'a Process {
        self.process
    }

    /// Whether the task is the process's thread group leader: the whole
    /// process, or its main thread. On Linux, only the leader's thread ID
    /// equals the process ID.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert!(process.linux_task().is_leader());
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub const fn is_leader(&self) -> bool {
        match self.kind {
            Kind::Process => true,
            Kind::Thread { main, .. } => main,
        }
    }

    /// The task's ID: the Darwin thread ID of a thread, or the process ID.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(
    ///     process.linux_task().id(),
    ///     u64::from(Pid::current().as_raw().unsigned_abs())
    /// );
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn id(&self) -> u64 {
        match self.kind {
            Kind::Process => u64::from(self.process.info.pid.as_raw().unsigned_abs()),
            Kind::Thread { thread, .. } => thread.id,
        }
    }

    /// The task's name as procps-ng's `CMD` item shows it. The main thread
    /// has the process's name from `p_comm`, even when the thread has set its
    /// own name. On Linux, a name that the main thread sets becomes the
    /// process's name. Many macOS programs set a name for their main thread,
    /// and Rust's runtime gives it the name `main`, so the process's name stays
    /// `p_comm`.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Charset, Field, Process};
    ///
    /// let mut info = Pid::current().info()?;
    /// info.comm = "worker".into();
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_task().command(Charset::Utf8), "worker");
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn command(&self, charset: Charset) -> OsString {
        match self.kind {
            Kind::Thread {
                thread,
                main: false,
            } => self.process.linux_thread_command(thread, charset),
            Kind::Process | Kind::Thread { main: true, .. } => self.process.linux_command(charset),
        }
    }

    /// The task's command line as procps-ng's `CMDLINE` item shows it. The
    /// threads of a process share its arguments. When the process has no
    /// arguments, or the source could not read them, the command line is the
    /// task's own name in brackets, as procps-ng shows it for a Linux thread.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Charset, Field, Process};
    ///
    /// let mut info = Pid::current().info()?;
    /// info.comm = "worker".into();
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_task().command_line(Charset::Utf8), "[worker]");
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn command_line(&self, charset: Charset) -> OsString {
        self.process
            .linux_command_line_or(&self.command(charset), charset)
    }

    /// The task's state letter, or `None` if the source could not read the
    /// data that decides it.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_task().state_letter(), None);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn state_letter(&self) -> Option<char> {
        match self.kind {
            Kind::Process => self.process.linux_state().known_letter(),
            Kind::Thread { thread, .. } => Some(self.process.linux_thread_state(thread)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use darwin_proc::{RunState, ThreadInfo};
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use crate::{
        Charset, Field, Process,
        testing::{self, thread},
    };

    /// A process called `worker` with the pid 10 and the threads `threads`.
    fn process(threads: Option<Field<Vec<ThreadInfo>>>) -> Process {
        let mut process = testing::process(10, "worker");
        process.threads = threads;

        process
    }

    /// The ID, name and state letter of each task.
    fn summary(process: &Process, charset: Charset) -> Vec<(u64, OsString, Option<char>)> {
        process
            .linux_tasks()
            .iter()
            .map(|task| (task.id(), task.command(charset), task.state_letter()))
            .collect()
    }

    #[test]
    fn threads_are_tasks_in_ascending_order_of_id() {
        let process = process(Some(Field::Available(vec![
            thread(30, b"io", RunState::Running),
            thread(10, b"main", RunState::Waiting),
            thread(20, b"", RunState::Uninterruptible),
        ])));

        assert_eq!(
            summary(&process, Charset::Utf8),
            [
                (10, OsString::from("worker"), Some('S')),
                (20, OsString::from("worker"), Some('D')),
                (30, OsString::from("io"), Some('R')),
            ]
        );
    }

    #[test]
    fn a_thread_name_keeps_its_bytes_until_it_is_escaped() {
        let process = process(Some(Field::Available(vec![
            thread(1, b"main", RunState::Running),
            thread(2, b"bad\xff\xfe", RunState::Running),
        ])));

        assert_eq!(
            summary(&process, Charset::Utf8),
            [
                (1, OsString::from("worker"), Some('R')),
                (2, OsString::from("bad??"), Some('R')),
            ]
        );
    }

    #[rstest]
    #[case::unreadable(Some(Field::Denied), ["[worker]", "[worker]", "[io]"])]
    #[case::readable(
        Some(Field::Available(vec![OsString::from("worker"), OsString::from("-v")])),
        ["worker -v", "worker -v", "worker -v"],
    )]
    fn a_task_s_command_line_falls_back_to_its_own_name(
        #[case] arguments: Option<Field<Vec<OsString>>>,
        #[case] expected: [&str; 3],
    ) {
        let mut process = process(Some(Field::Available(vec![
            thread(1, b"main", RunState::Running),
            thread(2, b"", RunState::Running),
            thread(3, b"io", RunState::Running),
        ])));
        process.arguments = arguments;

        let command_lines: Vec<OsString> = process
            .linux_tasks()
            .iter()
            .map(|task| task.command_line(Charset::Utf8))
            .collect();

        assert_eq!(command_lines, expected.map(OsString::from));
    }

    // With an empty list of threads, the process's state is known.
    #[rstest]
    #[case::denied(Some(Field::Denied), None)]
    #[case::not_requested(None, None)]
    #[case::empty(Some(Field::Available(Vec::new())), Some('S'))]
    fn a_process_without_known_threads_is_one_task(
        #[case] threads: Option<Field<Vec<ThreadInfo>>>,
        #[case] state: Option<char>,
    ) {
        let process = process(threads);

        assert_eq!(
            (summary(&process, Charset::Utf8), process.linux_task().id()),
            (vec![(10, OsString::from("worker"), state)], 10)
        );
    }
}
