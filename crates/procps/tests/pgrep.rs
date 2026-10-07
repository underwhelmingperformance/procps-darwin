// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs `pgrep` and `pkill` against child processes with known arguments,
//! environment, process group, session and terminal, on the live system.
//!
//! The reference harness compares the tools with procps-ng, but its fixtures
//! cannot set the arguments, process group, session or terminal of a process.
//! Each child is this test binary, running the ignored test `child` or
//! `session_leader` with a marker that no other process has. Each test selects
//! by the marker, or by a process group, session or terminal that only its
//! children have. The age test also selects launchd by its pid. The tests can
//! therefore run in parallel with each other and alongside other processes on
//! the system. The environment and age tests, and the process group test for
//! `-P`, also run a query that must select no process. The process group,
//! session and start-order tests check that a query selects some of their
//! children and leaves out the others.

use std::{
    fmt::Write as _,
    io::{self, BufRead, BufReader, Read, Write},
    os::{
        fd::AsFd,
        unix::process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{ChildStdout, Command, Stdio},
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

use assert_matches::assert_matches;
use darwin_proc::{Call, Host, Pid};
use pretty_assertions::assert_eq;
use rstest::rstest;

/// The variable that makes the ignored tests run as children. In a child, the
/// variable's value is the child's marker. Without the variable, the ignored
/// tests return at once.
const MARKER: &str = "PROCPS_DARWIN_TEST_CHILD";

/// The start of the line that a child writes once it is ready.
const READY: &str = "procps-darwin child ready:";

/// How long a child has to report that it is ready, enough for a heavily loaded
/// machine.
const READY_LIMIT: Duration = Duration::from_secs(60);

/// How long a test waits for a child to end once it has signalled the child.
const END_LIMIT: Duration = Duration::from_secs(60);

/// A counter that distinguishes this process's children.
static NEXT_MARKER: AtomicU32 = AtomicU32::new(1);

/// A marker that no other process uses.
fn unique_marker() -> String {
    let count = NEXT_MARKER.fetch_add(1, Ordering::Relaxed);

    child_marker(std::env::var(MARKER).ok().as_deref(), Pid::current(), count)
}

/// The marker of the `count`th child of the process with pid `pid`, whose own
/// marker is `inherited` if it runs as a child. The test binary, when it does
/// not run as a child, gives its children its pid followed by a counter, and a
/// child gives its own children its marker followed by a counter. Every marker
/// in a run therefore starts with [`marker_prefix`] of the first test binary's
/// pid.
fn child_marker(inherited: Option<&str>, pid: Pid, count: u32) -> String {
    match inherited {
        Some(marker) => format!("{marker}-{count}"),
        None => format!("{}{count}", marker_prefix(pid)),
    }
}

#[test]
fn the_markers_of_a_child_s_children_differ_from_the_root_s() {
    let first = child_marker(None, Pid::from(123), 1);

    assert_eq!(
        [
            child_marker(Some(&first), Pid::from(456), 1),
            child_marker(None, Pid::from(123), 11),
        ],
        [
            "procps-darwin-test-123-1-1".to_owned(),
            "procps-darwin-test-123-11".to_owned()
        ]
    );
}

/// The start of every marker in the run of the test binary with pid `root`.
fn marker_prefix(root: Pid) -> String {
    format!("procps-darwin-test-{root}-")
}

/// Whether a process with `arguments` belongs to the run of the test binary at
/// `executable` with pid `root`. The process must run the test binary, and one
/// of its arguments must start with [`marker_prefix`] of `root`. A child of the
/// test keeps its marker after its parent has exited.
fn is_test_process(arguments: &darwin_proc::Arguments, executable: &Path, root: Pid) -> bool {
    let prefix = marker_prefix(root);

    arguments.executable == executable
        && arguments
            .arguments
            .iter()
            .any(|argument| argument.as_encoded_bytes().starts_with(prefix.as_bytes()))
}

#[rstest]
#[case::child("/test", "procps-darwin-test-123-4", true)]
#[case::child_of_a_child("/test", "procps-darwin-test-123-4-1", true)]
#[case::longer_pid("/test", "procps-darwin-test-1234-1", false)]
#[case::other_pid("/test", "procps-darwin-test-456-1", false)]
#[case::marker_inside_an_argument("/test", " procps-darwin-test-123-4$", false)]
#[case::other_executable("/usr/bin/pgrep", "procps-darwin-test-123-4", false)]
fn a_test_process_runs_the_test_binary_with_a_marker_of_the_run(
    #[case] executable: &str,
    #[case] marker: &str,
    #[case] expected: bool,
) {
    let arguments = darwin_proc::Arguments {
        executable: PathBuf::from(executable),
        arguments: vec![executable.into(), "--exact".into(), marker.into()],
        environment: Vec::new(),
    };

    assert_eq!(
        is_test_process(&arguments, Path::new("/test"), Pid::from(123)),
        expected
    );
}

/// What a child process runs.
#[derive(Clone, Copy, Debug)]
enum Role {
    /// [`child`], which waits for its standard input to close.
    Plain,
    /// [`session_leader`], which also leads a new session with a terminal.
    SessionLeader,
    /// [`silent_child`], which waits for its standard input to close without
    /// reporting that it is ready.
    Silent,
    /// [`leave_an_orphan`], which starts a child of this run and exits.
    LeaveAnOrphan,
    /// [`leave_a_decoy`], which starts a child of no run and exits.
    LeaveADecoy,
    /// A test name that matches no test, so the child exits at once.
    Absent,
}

impl Role {
    const fn test(self) -> &'static str {
        match self {
            Self::Plain => "child",
            Self::SessionLeader => "session_leader",
            Self::Silent => "silent_child",
            Self::LeaveAnOrphan => "leave_an_orphan",
            Self::LeaveADecoy => "leave_a_decoy",
            Self::Absent => "no_such_test",
        }
    }
}

/// Why a child process did not become ready.
#[derive(Debug, thiserror::Error)]
enum ChildError {
    #[error("cannot start the child or read its output")]
    Io(#[from] io::Error),
    #[error("the child has no pipe for its standard output or standard error")]
    NoPipe,
    #[error(
        "the child exited before it was ready; its standard output was {stdout:?}, and its \
         standard error was {stderr:?}"
    )]
    ExitedBeforeReady { stdout: String, stderr: String },
    #[error(
        "the child was not ready within {after:?}, so the test killed it; its standard output \
         was {stdout:?}, and its standard error was {stderr:?}"
    )]
    NotReadyInTime {
        after: Duration,
        stdout: String,
        stderr: String,
    },
    #[error("the child did not end within {after:?}")]
    DidNotEnd { after: Duration },
}

/// A child process that runs one of this test binary's ignored tests. The child
/// is killed and reaped when the value is dropped.
struct Child {
    process: std::process::Child,
    /// Kept open, so that the child never writes to a closed pipe.
    stdout: BufReader<ChildStdout>,
    marker: String,
    /// What the child wrote after [`READY`].
    report: String,
}

impl Child {
    /// Starts a child in `role`, after `configure` has changed its command, and
    /// returns once the child is ready.
    fn start(role: Role, configure: impl FnOnce(&mut Command)) -> Result<Self, ChildError> {
        Self::start_within(role, configure, READY_LIMIT)
    }

    /// Starts a child in `role`, after `configure` has changed its command, and
    /// returns once the child is ready. A child that is not ready within
    /// `limit` is killed.
    fn start_within(
        role: Role,
        configure: impl FnOnce(&mut Command),
        limit: Duration,
    ) -> Result<Self, ChildError> {
        let marker = unique_marker();
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["--ignored", "--exact", "--quiet", role.test(), &marker])
            .env(MARKER, &marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure(&mut command);

        let mut process = command.spawn()?;
        let (Some(stdout), Some(mut stderr)) = (process.stdout.take(), process.stderr.take())
        else {
            let _ = process.kill();
            let _ = process.wait();
            return Err(ChildError::NoPipe);
        };
        let mut child = Self {
            process,
            stdout: BufReader::new(stdout),
            marker,
            report: String::new(),
        };

        let ready = ready_report(&mut child.stdout, Instant::now() + limit)?;

        if let Ready::Report(report) = ready {
            child.report = report;
            return Ok(child);
        }

        // A child that is still running keeps its standard error open.
        let _ = child.process.kill();
        child.process.wait()?;
        let mut text = String::new();
        stderr.read_to_string(&mut text)?;

        Err(match ready {
            Ready::TimedOut(stdout) => ChildError::NotReadyInTime {
                after: limit,
                stdout,
                stderr: text,
            },
            Ready::Exited(stdout) | Ready::Report(stdout) => ChildError::ExitedBeforeReady {
                stdout,
                stderr: text,
            },
        })
    }

    /// Waits at most `limit` for the child to end, and returns how it ended.
    /// The child's standard output closes when the child ends, so the test
    /// waits for the end of that output with `poll` and a deadline.
    fn wait_for_end(&mut self, limit: Duration) -> Result<std::process::ExitStatus, ChildError> {
        let deadline = Instant::now() + limit;

        loop {
            if self.stdout.buffer().is_empty() {
                let left = deadline.saturating_duration_since(Instant::now());
                let wait = rustix::event::Timespec::try_from(left).map_err(io::Error::other)?;
                let mut ready = [rustix::event::PollFd::new(
                    self.stdout.get_ref(),
                    rustix::event::PollFlags::IN,
                )];

                if rustix::event::poll(&mut ready, Some(&wait)).map_err(io::Error::from)? == 0 {
                    return Err(ChildError::DidNotEnd { after: limit });
                }
            }

            let length = self.stdout.fill_buf()?.len();

            if length == 0 {
                return Ok(self.process.wait()?);
            }

            self.stdout.consume(length);

            if Instant::now() >= deadline {
                return Err(ChildError::DidNotEnd { after: limit });
            }
        }
    }

    fn new() -> Result<Self, ChildError> {
        Self::start(Role::Plain, |_| {})
    }

    fn pid(&self) -> Result<Pid, std::num::TryFromIntError> {
        Ok(Pid::from(i32::try_from(self.process.id())?))
    }

    /// The pattern that matches the child's command line with `-f`.
    fn pattern(&self) -> String {
        pattern_for(&self.marker)
    }
}

/// The pattern that matches, with `-f`, the command line of a child with
/// `marker`. `-f` joins the arguments with spaces, so the pattern matches only
/// a command line whose last argument is `marker`.
fn pattern_for(marker: &str) -> String {
    format!(" {marker}$")
}

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// What a child wrote before it was ready, exited or ran out of time.
#[derive(Debug, PartialEq, Eq)]
enum Ready {
    /// The text after [`READY`].
    Report(String),
    /// The child closed its standard output before it was ready, after writing
    /// this.
    Exited(String),
    /// The deadline passed before the child was ready, after it had written
    /// this.
    TimedOut(String),
}

/// What the child wrote on the line that starts with [`READY`], if it writes
/// the line before `deadline`. libtest writes its own lines first. Once the
/// deadline has passed, the read returns what it has read, even if more output
/// is available, so a child cannot make it run past the deadline.
fn ready_report<R: Read + AsFd>(stdout: &mut BufReader<R>, deadline: Instant) -> io::Result<Ready> {
    let mut written = Vec::new();
    let mut start = 0;

    loop {
        if stdout.buffer().is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            let wait = rustix::event::Timespec::try_from(left).map_err(io::Error::other)?;
            let mut ready = [rustix::event::PollFd::new(
                stdout.get_ref(),
                rustix::event::PollFlags::IN,
            )];

            if rustix::event::poll(&mut ready, Some(&wait))? == 0 {
                return Ok(Ready::TimedOut(
                    String::from_utf8_lossy(&written).into_owned(),
                ));
            }
        }

        let available = stdout.fill_buf()?;

        if available.is_empty() {
            return Ok(Ready::Exited(
                String::from_utf8_lossy(&written).into_owned(),
            ));
        }

        if let Some(end) = available.iter().position(|&byte| byte == b'\n') {
            written.extend_from_slice(&available[..=end]);
            stdout.consume(end + 1);
            let line = String::from_utf8_lossy(&written[start..]).into_owned();
            start = written.len();

            if let Some(report) = line.trim_end().strip_prefix(READY) {
                return Ok(Ready::Report(report.trim_start().to_owned()));
            }
        } else {
            let length = available.len();
            written.extend_from_slice(available);
            stdout.consume(length);
        }

        if Instant::now() >= deadline {
            return Ok(Ready::TimedOut(
                String::from_utf8_lossy(&written).into_owned(),
            ));
        }
    }
}

// A deadline that has passed returns what has arrived. A later deadline is
// never reached, because each case ends before it.
#[rstest]
#[case::report(
    b"running 1 test\nprocps-darwin child ready: x\n",
    true,
    false,
    Ready::Report("x".to_owned())
)]
#[case::other_lines(
    b"running 1 test\n",
    true,
    true,
    Ready::TimedOut("running 1 test\n".to_owned())
)]
#[case::partial_line(b"procps-dar", true, true, Ready::TimedOut("procps-dar".to_owned()))]
#[case::closed(b"running 1 test\n", false, false, Ready::Exited("running 1 test\n".to_owned()))]
#[case::more_lines_after_the_deadline(
    b"running 1 test\nrunning 1 test\nrunning 1 test\n",
    true,
    true,
    Ready::TimedOut("running 1 test\n".to_owned())
)]
fn a_child_is_ready_only_if_it_reports_before_the_deadline(
    #[case] written: &[u8],
    #[case] stays_open: bool,
    #[case] deadline_passed: bool,
    #[case] expected: Ready,
) -> io::Result<()> {
    let (reader, mut writer) = io::pipe()?;
    writer.write_all(written)?;
    let open = stays_open.then_some(writer);
    let deadline = if deadline_passed {
        Instant::now()
    } else {
        Instant::now() + Duration::from_secs(60)
    };

    assert_eq!(
        ready_report(&mut BufReader::with_capacity(16, reader), deadline)?,
        expected
    );

    drop(open);
    Ok(())
}

#[test]
fn a_child_that_exits_or_stays_silent_is_not_ready() {
    assert_matches!(
        (
            Child::start(Role::Absent, |_| {}).map(drop),
            Child::start_within(Role::Silent, |_| {}, Duration::ZERO).map(drop),
        ),
        (
            Err(ChildError::ExitedBeforeReady { .. }),
            Err(ChildError::NotReadyInTime {
                after: Duration::ZERO,
                ..
            }),
        )
    );
}

#[test]
fn a_child_that_does_not_end_in_time_is_an_error() -> Result<(), ChildError> {
    let mut child = Child::new()?;

    assert_matches!(
        child.wait_for_end(Duration::ZERO),
        Err(ChildError::DidNotEnd {
            after: Duration::ZERO
        })
    );

    Ok(())
}

/// Reports that a child is ready, then waits until its standard input closes.
fn serve(report: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{READY} {report}")?;
    stdout.flush()?;

    io::copy(&mut io::stdin().lock(), &mut io::sink())?;

    Ok(())
}

/// Runs as a child of the other tests.
#[test]
#[ignore = "the other tests run it as a child"]
fn child() -> io::Result<()> {
    if std::env::var_os(MARKER).is_none() {
        return Ok(());
    }

    serve("")
}

/// Runs as a child that never reports that it is ready.
#[test]
#[ignore = "a_child_that_exits_or_stays_silent_is_not_ready runs it as a child"]
fn silent_child() -> io::Result<()> {
    if std::env::var_os(MARKER).is_none() {
        return Ok(());
    }

    io::copy(&mut io::stdin().lock(), &mut io::sink())?;

    Ok(())
}

/// Runs as a child that leads a new session with a pseudo-terminal as its
/// controlling terminal. The child starts a member of the session in a new
/// process group, then reports the terminal's name and the member's pid, such
/// as `/dev/ttys004 12345`. The member exits soon after the leader, once its
/// standard input closes.
#[test]
#[ignore = "the session test runs it as a child"]
fn session_leader() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os(MARKER).is_none() {
        return Ok(());
    }

    rustix::process::setsid()?;

    let controller =
        rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)?;
    rustix::pty::grantpt(&controller)?;
    rustix::pty::unlockpt(&controller)?;
    let name = rustix::pty::ptsname(&controller, Vec::new())?;
    let terminal = rustix::fs::open(
        name.as_c_str(),
        rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NOCTTY,
        rustix::fs::Mode::empty(),
    )?;
    rustix::process::ioctl_tiocsctty(&terminal)?;

    let member = Child::start(Role::Plain, |command| {
        command.process_group(0);
    })?;

    Ok(serve(&format!("{} {}", name.to_str()?, member.pid()?))?)
}

/// The standard output, standard error and exit status of a tool.
type Outcome = (String, String, Option<i32>);

/// Runs `tool` with `arguments`, and sets `PROCPS_DARWIN_SOURCE=local` so
/// that the tool reads process data itself, not through procps-helperd. The
/// tool runs without `PROCPS_DARWIN_LOG`, so it writes no traces to standard
/// error. It runs with `LC_ALL=C.UTF-8`, so it prints a command line in UTF-8
/// without escaping it, whatever the caller's locale.
fn run(tool: &str, arguments: &[&str]) -> io::Result<Outcome> {
    let output = Command::new(tool)
        .args(arguments)
        .env("PROCPS_DARWIN_SOURCE", "local")
        .env("LC_ALL", "C.UTF-8")
        .env_remove("PROCPS_DARWIN_LOG")
        .output()?;

    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    ))
}

fn pgrep(arguments: &[&str]) -> io::Result<Outcome> {
    run(env!("CARGO_BIN_EXE_pgrep"), arguments)
}

/// What `pgrep` prints when it selects `pids`: one line for each pid, in
/// ascending order, and status 0, or status 1 if `pids` is empty.
fn selected(pids: &[Pid]) -> Outcome {
    let mut pids: Vec<i32> = pids.iter().map(|pid| pid.as_raw()).collect();
    pids.sort_unstable();
    let stdout = pids.iter().fold(String::new(), |mut stdout, pid| {
        let _ = writeln!(stdout, "{pid}");
        stdout
    });

    (stdout, String::new(), Some(i32::from(pids.is_empty())))
}

#[test]
fn a_child_is_selected_and_listed_by_its_command_line() -> Result<(), Box<dyn std::error::Error>> {
    let child = Child::new()?;
    let pid = child.pid()?;
    let program = std::env::current_exe()?;
    let suffix = &child.marker[1..];

    assert_eq!(
        (
            pgrep(&["-f", &child.pattern()])?,
            pgrep(&["-a", "-f", &child.pattern()])?,
            pgrep(&["-f", &pattern_for(suffix)])?,
        ),
        (
            selected(&[pid]),
            (
                format!(
                    "{pid} {} --ignored --exact --quiet child {}\n",
                    program.display(),
                    child.marker
                ),
                String::new(),
                Some(0)
            ),
            selected(&[])
        )
    );

    Ok(())
}

#[test]
fn a_child_is_selected_by_its_environment() -> Result<(), Box<dyn std::error::Error>> {
    let child = Child::new()?;
    let parent = std::process::id().to_string();
    let entry = format!("{MARKER}={}", child.marker);
    let other = format!("{MARKER}=other");

    assert_eq!(
        (
            pgrep(&["--env", &entry, "-P", &parent])?,
            pgrep(&["--env", &other, "-P", &parent])?,
            pgrep(&["--env", &entry, "-P", "1"])?,
        ),
        (selected(&[child.pid()?]), selected(&[]), selected(&[]))
    );

    Ok(())
}

// The group's leader and member are the only processes in their new process
// group, so `-g` needs no pattern.
#[test]
fn children_are_selected_by_their_process_group_and_parent()
-> Result<(), Box<dyn std::error::Error>> {
    let leader = Child::start(Role::Plain, |command| {
        command.process_group(0);
    })?;
    let group = leader.pid()?;
    let member = Child::start(Role::Plain, |command| {
        command.process_group(group.as_raw());
    })?;
    let other = Child::new()?;
    let all = format!(
        "{}|{}|{}",
        leader.pattern(),
        member.pattern(),
        other.pattern()
    );
    let parent = std::process::id().to_string();

    assert_eq!(
        (
            pgrep(&["-g", &group.to_string()])?,
            pgrep(&["-P", &parent, "-f", &all])?,
            pgrep(&["-P", "1", "-f", &all])?,
        ),
        (
            selected(&[group, member.pid()?]),
            selected(&[group, member.pid()?, other.pid()?]),
            selected(&[])
        )
    );

    Ok(())
}

/// The name of the terminal of the session that `leader` leads, without
/// `/dev/`, and the pid of the session's member.
fn session_of(leader: &Child) -> Result<(&str, Pid), Box<dyn std::error::Error>> {
    let (terminal, member) = leader
        .report
        .split_once(' ')
        .ok_or("the session leader did not report its member")?;

    Ok((
        terminal.trim_start_matches("/dev/"),
        Pid::from(member.parse::<i32>()?),
    ))
}

// The leader and its member are the only processes in their new session and on
// their new terminal, so the queries need no pattern. The member's marker
// starts with the leader's marker, so the leader's pattern must not select the
// member.
#[test]
fn children_are_selected_by_their_session_and_terminal() -> Result<(), Box<dyn std::error::Error>> {
    let leader = Child::start(Role::SessionLeader, |_| {})?;
    let (terminal, member) = session_of(&leader)?;
    let session = leader.pid()?;

    assert_eq!(
        (
            pgrep(&["-t", terminal])?,
            pgrep(&["-s", &session.to_string()])?,
            pgrep(&["-g", &session.to_string()])?,
            pgrep(&["-f", &leader.pattern()])?,
        ),
        (
            selected(&[session, member]),
            selected(&[session, member]),
            selected(&[session]),
            selected(&[session])
        )
    );

    Ok(())
}

// The member of a session is a child of the session's leader, not of this
// process, and it keeps running for a moment after a test has killed the
// leader.
#[test]
fn the_leader_and_member_of_a_session_are_test_processes() -> Result<(), Box<dyn std::error::Error>>
{
    let leader = Child::start(Role::SessionLeader, |_| {})?;
    let (_, member) = session_of(&leader)?;
    let executable = std::env::current_exe()?;
    let arguments = [leader.pid()?.arguments()?, member.arguments()?];

    assert_eq!(
        arguments.map(|arguments| is_test_process(&arguments, &executable, Pid::current())),
        [true, true]
    );

    Ok(())
}

// The tools compare start times in clock ticks of 10 ms, and two children can
// start in the same tick. procps-ng then keeps the higher pid for `-n` and the
// lower pid for `-o`, so the second child must have the higher pid. If the pids
// wrap between the two starts, the second child becomes the first, and the
// test starts another.
#[test]
fn the_newest_and_oldest_children_are_selected() -> Result<(), Box<dyn std::error::Error>> {
    let mut first = Child::new()?;
    let mut second = Child::new()?;

    while second.pid()? < first.pid()? {
        first = second;
        second = Child::new()?;
    }

    let pattern = format!("{}|{}", first.pattern(), second.pattern());

    assert_eq!(
        (
            pgrep(&["-f", "-o", &pattern])?,
            pgrep(&["-f", "-n", &pattern])?,
        ),
        (selected(&[first.pid()?]), selected(&[second.pid()?]))
    );

    Ok(())
}

/// A `sandbox-exec` profile that lets a process signal only the processes in
/// its own sandbox. A process inherits the sandbox of the process that started
/// it.
const SIGNALS_STAY_IN_THE_SANDBOX: &str =
    "(version 1) (allow default) (deny signal) (allow signal (target same-sandbox))";

/// What `sandbox-exec` writes when macOS refuses to apply its profile. macOS
/// refuses to apply [`SIGNALS_STAY_IN_THE_SANDBOX`] under some sandbox
/// profiles, including the profile that Nix applies to each build on macOS and
/// [`SIGNALS_STAY_IN_THE_OUTER_SANDBOX`].
const NESTED_SANDBOX_REFUSAL: &str = "sandbox-exec: sandbox_apply: Operation not permitted\n";

/// The status with which `sandbox-exec` exits when macOS refuses to apply its
/// profile.
const NESTED_SANDBOX_STATUS: i32 = 71;

/// Whether `output` is that of a `sandbox-exec` that macOS refused to apply.
fn refused(output: &std::process::Output) -> bool {
    output.status.code() == Some(NESTED_SANDBOX_STATUS)
        && output.stderr == NESTED_SANDBOX_REFUSAL.as_bytes()
}

#[test]
fn only_the_refusal_to_nest_a_sandbox_is_a_refusal() {
    let output = |code: i32, stderr: &str| std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    };

    assert_eq!(
        [
            refused(&output(71, NESTED_SANDBOX_REFUSAL)),
            refused(&output(1, NESTED_SANDBOX_REFUSAL)),
            refused(&output(71, "sandbox-exec: unbound variable\n")),
            refused(&output(0, "")),
        ],
        [true, false, false, false]
    );
}

/// The variable that the Nix build of the tests sets. macOS refuses the
/// scenarios' sandboxes in the build's sandbox, and the scenarios skip only
/// when this variable is set.
const NIX_BUILD: &str = "PROCPS_DARWIN_TEST_NIX_BUILD";

/// A `sandbox-exec` profile that lets a process signal only the processes in
/// its own sandbox, as [`SIGNALS_STAY_IN_THE_SANDBOX`] does, and that also
/// denies network access, which the tests do not use. macOS refuses to apply
/// [`SIGNALS_STAY_IN_THE_SANDBOX`] under this profile.
const SIGNALS_STAY_IN_THE_OUTER_SANDBOX: &str = "(version 1) (allow default) (deny signal) (allow \
                                                 signal (target same-sandbox)) (deny network*)";

/// The variable that tells [`pkill_signals_a_child`] what confines its signals.
const CONFINEMENT: &str = "PROCPS_DARWIN_TEST_CONFINEMENT";

/// What confines the signals of [`pkill_signals_a_child`] and of the `pkill`
/// that it runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Confinement {
    /// A sandbox that lets a process signal only the processes in it.
    Sandbox,
    /// No sandbox. Just before the test started, [`signalable_outside`] found
    /// no process outside the test that accepted signal 0.
    CheckedProcessTable,
}

impl Confinement {
    const ALL: [Self; 2] = [Self::Sandbox, Self::CheckedProcessTable];

    const fn name(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::CheckedProcessTable => "checked-process-table",
        }
    }
}

/// A value of [`CONFINEMENT`] that is not the name of a [`Confinement`].
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
#[error("{CONFINEMENT} is {0:?}, which is not one of {names:?}", names = Confinement::ALL.map(Confinement::name))]
struct UnknownConfinement(String);

impl std::str::FromStr for Confinement {
    type Err = UnknownConfinement;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|confinement| confinement.name() == name)
            .ok_or_else(|| UnknownConfinement(name.to_owned()))
    }
}

#[rstest]
#[case::sandbox("sandbox", Ok(Confinement::Sandbox))]
#[case::checked_process_table("checked-process-table", Ok(Confinement::CheckedProcessTable))]
#[case::other_case("Sandbox", Err(UnknownConfinement("Sandbox".to_owned())))]
#[case::empty("", Err(UnknownConfinement(String::new())))]
fn only_the_name_of_a_confinement_is_a_confinement(
    #[case] name: &str,
    #[case] expected: Result<Confinement, UnknownConfinement>,
) {
    assert_eq!(name.parse::<Confinement>(), expected);
}

/// Why the pkill test could not run, or failed.
#[derive(Debug, thiserror::Error)]
enum PkillTestError {
    #[error("cannot find or start the test binary")]
    Io(#[from] io::Error),
    #[error("cannot read the process table or signal a process")]
    Process(#[from] darwin_proc::Error),
    #[error("the pkill test under {confinement:?} did not pass: {run:?}")]
    NotPassed {
        confinement: Confinement,
        run: TestRun,
    },
    #[error(
        "sandbox-exec cannot confine pkill, because the test already runs in a sandbox, and \
         these processes outside the test accept signals from the test: {0:?}. A \
         selection bug in pkill could signal them, so the test does not run pkill without a \
         sandbox"
    )]
    Unconfined(Vec<Pid>),
}

/// The processes in `processes` that accept a signal according to `probe`,
/// apart from the processes of the test. Each entry of `processes` is a pid and
/// its parent's pid. The function does not probe `test`, its ancestors or its
/// descendants. It probes every other process, and leaves out a process that
/// refuses the signal or has exited, and a process for which `ours` is true. It
/// returns any other error from `probe`, so a probe that fails for another
/// reason does not count as a refusal.
fn signalable_outside(
    processes: &[(Pid, Pid)],
    test: Pid,
    probe: impl Fn(Pid) -> Result<(), darwin_proc::Error>,
    ours: impl Fn(Pid) -> bool,
) -> Result<Vec<Pid>, darwin_proc::Error> {
    let parent = |pid: Pid| {
        processes
            .iter()
            .find(|&&(candidate, _)| candidate == pid)
            .map(|&(_, parent)| parent)
    };
    let mut tests = vec![test];
    let mut ancestor = parent(test);

    while let Some(pid) = ancestor.filter(|pid| !tests.contains(pid)) {
        tests.push(pid);
        ancestor = parent(pid);
    }

    let mut descendants = vec![test];
    let mut grown = true;

    while grown {
        grown = false;

        for &(pid, parent) in processes {
            if descendants.contains(&parent) && !descendants.contains(&pid) {
                descendants.push(pid);
                grown = true;
            }
        }
    }

    tests.extend(descendants);
    let mut signalable = Vec::new();

    for &(pid, _) in processes {
        if tests.contains(&pid) {
            continue;
        }

        let accepts = match probe(pid) {
            Ok(()) => true,
            Err(darwin_proc::Error::Denied { .. } | darwin_proc::Error::Exited { .. }) => false,
            Err(error) => return Err(error),
        };

        if accepts && !ours(pid) {
            signalable.push(pid);
        }
    }

    Ok(signalable)
}

/// Sends signal 0 to `pid`. A zombie accepts signal 0 from every process, even
/// from a sandbox, because `kill(2)` reports success for a zombie without
/// checking permissions. The kernel discards every signal to a process that is
/// exiting, which includes a zombie, so the probe reports such a process as one
/// that has exited.
fn probe(pid: Pid) -> Result<(), darwin_proc::Error> {
    pid.send_signal(0)?;

    if pid.info()?.flags.is_exiting() {
        return Err(darwin_proc::Error::Exited { pid });
    }

    Ok(())
}

#[test]
fn a_zombie_does_not_accept_the_probe() -> Result<(), Box<dyn std::error::Error>> {
    let live = Child::new()?;
    let mut exited = Command::new(std::env::current_exe()?)
        .args(["--exact", Role::Absent.test()])
        .stdout(Stdio::null())
        .spawn()?;
    rustix::process::waitid(
        rustix::process::WaitId::Pid(rustix::process::Pid::from_child(&exited)),
        rustix::process::WaitIdOptions::EXITED | rustix::process::WaitIdOptions::NOWAIT,
    )?;
    let zombie = Pid::from(i32::try_from(exited.id())?);
    let outcomes = [probe(live.pid()?), probe(zombie), probe(Pid::from(1))];
    exited.wait()?;

    assert_matches!(
        outcomes,
        [
            Ok(()),
            Err(darwin_proc::Error::Exited { pid }),
            Err(darwin_proc::Error::Denied {
                call: Call::Signal,
                ..
            }),
        ] if pid == zombie
    );

    Ok(())
}

/// What [`signalable_outside`] returns for a test with pid 10 in `processes`,
/// which lists pids and their parents' pids. Process 21 refuses signals, 22 has
/// exited, 23's probe fails with another error, and 30 belongs to the test.
/// Every other process accepts signals.
fn signalable_in(processes: &[(i32, i32)]) -> Result<Vec<Pid>, darwin_proc::Error> {
    let processes: Vec<(Pid, Pid)> = processes
        .iter()
        .map(|&(pid, parent)| (Pid::from(pid), Pid::from(parent)))
        .collect();
    let probe = |target: Pid| match target.as_raw() {
        21 => Err(darwin_proc::Error::Denied {
            pid: target,
            call: Call::Signal,
        }),
        22 => Err(darwin_proc::Error::Exited { pid: target }),
        23 => Err(darwin_proc::Error::Os {
            call: Call::Signal,
            source: io::Error::from_raw_os_error(libc::EINVAL),
        }),
        _ => Ok(()),
    };

    signalable_outside(&processes, Pid::from(10), probe, |target| {
        target.as_raw() == 30
    })
}

// 5 is the test's parent, 11 its child and 13 its sibling. In the last case,
// 12 comes before its parent 11, so one pass over the table would miss it.
#[rstest]
#[case::outside_the_test(
    &[(5, 1), (10, 5), (11, 10), (13, 5), (20, 1), (21, 1), (22, 1), (30, 1)],
    &[13, 20]
)]
#[case::ancestors_above_the_parent(&[(3, 1), (4, 3), (5, 4), (10, 5), (20, 1)], &[20])]
#[case::kernel_task_is_its_own_parent(&[(0, 0), (1, 0), (5, 1), (10, 5), (20, 1)], &[20])]
#[case::descendant_before_its_parent(&[(12, 11), (11, 10), (10, 5), (20, 1)], &[20])]
fn only_processes_outside_the_test_that_accept_signals_are_signalable(
    #[case] processes: &[(i32, i32)],
    #[case] expected: &[i32],
) -> Result<(), darwin_proc::Error> {
    let expected: Vec<Pid> = expected.iter().copied().map(Pid::from).collect();

    assert_eq!(signalable_in(processes)?, expected);

    Ok(())
}

#[test]
fn a_probe_that_neither_succeeds_nor_is_refused_is_an_error() {
    assert_matches!(
        signalable_in(&[(10, 5), (20, 1), (23, 1)]),
        Err(darwin_proc::Error::Os {
            call: Call::Signal,
            ..
        })
    );
}

/// Runs [`pkill_signals_a_child`] as a child of this process, with
/// [`CONFINEMENT`] set to `confinement` or left out, and wraps the child's
/// command first if `wrap` is set.
fn run_pkill_test(
    confinement: Option<&str>,
    wrap: Option<&[&str]>,
) -> io::Result<std::process::Output> {
    let program = std::env::current_exe()?;
    let mut command = match wrap {
        Some(wrapper) => {
            let mut command = Command::new(wrapper[0]);
            command.args(&wrapper[1..]).arg(&program);
            command
        }
        None => Command::new(&program),
    };

    command
        .args(["--ignored", "--exact", "--quiet", "pkill_signals_a_child"])
        .env(MARKER, unique_marker())
        .env_remove(CONFINEMENT);

    if let Some(confinement) = confinement {
        command.env(CONFINEMENT, confinement);
    }

    command.output()
}

/// How a run of one of this test binary's tests ended.
#[derive(Debug, Eq, PartialEq)]
enum TestRun {
    /// libtest reported that one test passed.
    Passed,
    /// libtest exited with status 0, but reported no passed test. libtest does
    /// that when no test has the name that `--exact` gives.
    NoTestRan { stdout: String },
    /// libtest, or the program that ran it, failed.
    Failed {
        status: std::process::ExitStatus,
        stdout: String,
        stderr: String,
    },
}

impl From<&std::process::Output> for TestRun {
    fn from(output: &std::process::Output) -> Self {
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

        if !output.status.success() {
            return Self::Failed {
                status: output.status,
                stdout,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            };
        }

        if !stdout.contains("\ntest result: ok. 1 passed;") {
            return Self::NoTestRan { stdout };
        }

        Self::Passed
    }
}

#[test]
fn a_run_passes_only_if_libtest_reports_a_passed_test() {
    let output = |code: i32, stdout: &str| std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: b"error\n".to_vec(),
    };
    let passed = "\nrunning 1 test\n.\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 \
                  measured; 18 filtered out; finished in 0.56s\n\n";
    let none = "\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 \
                measured; 19 filtered out; finished in 0.00s\n\n";

    assert_eq!(
        [
            TestRun::from(&output(0, passed)),
            TestRun::from(&output(0, none)),
            TestRun::from(&output(101, passed)),
        ],
        [
            TestRun::Passed,
            TestRun::NoTestRan {
                stdout: none.to_owned()
            },
            TestRun::Failed {
                status: std::process::ExitStatus::from_raw(101 << 8),
                stdout: passed.to_owned(),
                stderr: "error\n".to_owned()
            },
        ]
    );
}

/// What `output`, from a run of the pkill test under `confinement`, means for
/// the test: `confinement` if the test passed, and otherwise an error.
fn passed(
    confinement: Confinement,
    output: &std::process::Output,
) -> Result<Confinement, PkillTestError> {
    match TestRun::from(output) {
        TestRun::Passed => Ok(confinement),
        run => Err(PkillTestError::NotPassed { confinement, run }),
    }
}

/// Runs [`pkill_signals_a_child`] with its signals confined, and returns what
/// confined them. `profile` is the profile for `sandbox-exec`.
///
/// A selection bug could make pkill send SIGKILL to every process of the user,
/// so the test confines pkill's signals in one of two ways. It first runs
/// `pkill_signals_a_child` under `sandbox-exec`, which starts the child and
/// pkill in the same sandbox. The kernel then refuses every signal to a process
/// outside it, whatever pkill selects.
///
/// macOS refuses to apply that profile under some sandbox profiles, including
/// the profile that Nix applies to each build on macOS, even with
/// `sandbox = false`. When `sandbox-exec` is refused for that reason, the test
/// reads the process table and sends signal 0 to every process except the test
/// binary, its ancestors and its descendants. It ignores a process that refuses
/// the signal or has exited, and a process that runs the test binary with a
/// marker of this run, such as the member of a session after a test has killed
/// the session's leader. It runs pkill without a sandbox only if no other
/// process accepts the signal. Otherwise it fails and lists the processes that
/// pkill could signal.
///
/// The check does not probe the test's ancestors, so a selection bug could end
/// them. That is safe only when each ancestor belongs to the test run or
/// refuses the user's signals. In a Nix build, cargo and the builder's shell
/// run as the build user, and the processes above them, such as the Nix daemon
/// and launchd, run as root. Under an outer sandbox of another kind, the
/// ancestors could include a CI service's runner.
///
/// The check cannot see a process that starts after it, or one that gains the
/// user's credentials after it, for example with `setuid`. It also does not
/// probe a new process that reuses the pid of one of the test's processes after
/// the test has read the process table. In a multi-user Nix installation, each
/// build runs as a build user of its own, and that user owns only the build's
/// processes. A process that the check misses can therefore accept pkill's
/// signals only if it belongs to the build. In a single-user installation, the
/// build runs as the user, whose other processes accept signal 0, so the check
/// fails.
fn confined_pkill_test(profile: &str) -> Result<Confinement, PkillTestError> {
    let output = run_pkill_test(
        Some(Confinement::Sandbox.name()),
        Some(&["/usr/bin/sandbox-exec", "-p", profile]),
    )?;

    if !refused(&output) {
        return passed(Confinement::Sandbox, &output);
    }

    let processes: Vec<(Pid, Pid)> = darwin_proc::ProcessInfo::all()?
        .into_iter()
        .map(|info| (info.pid, info.ppid))
        .collect();
    let executable = std::env::current_exe()?;
    let signalable = signalable_outside(&processes, Pid::current(), probe, |pid| {
        pid.arguments()
            .is_ok_and(|arguments| is_test_process(&arguments, &executable, Pid::current()))
    })?;

    if !signalable.is_empty() {
        return Err(PkillTestError::Unconfined(signalable));
    }

    passed(
        Confinement::CheckedProcessTable,
        &run_pkill_test(Some(Confinement::CheckedProcessTable.name()), None)?,
    )
}

#[test]
fn pkill_signals_the_selected_child() -> Result<(), PkillTestError> {
    confined_pkill_test(SIGNALS_STAY_IN_THE_SANDBOX).map(drop)
}

// `sandbox-exec` fails for a profile that it cannot compile, before it runs
// anything, so the test must not fall back to the check.
#[test]
fn a_sandbox_that_fails_otherwise_is_an_error() {
    assert_matches!(
        confined_pkill_test("(version 1) (no-such-rule)"),
        Err(PkillTestError::NotPassed {
            confinement: Confinement::Sandbox,
            run: TestRun::Failed { .. },
        })
    );
}

/// The variable that makes [`run_scenario`] run a [`Scenario`]. Its value is
/// the scenario's name.
const SCENARIO: &str = "PROCPS_DARWIN_TEST_SCENARIO";

/// What [`run_scenario`] checks, inside the sandbox that the test applies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    /// [`confined_pkill_test`] takes the fallback, and the check passes.
    Fallback,
    /// The check passes although the sandbox contains an orphaned child of
    /// this run.
    FallbackWithAnOrphan,
    /// The check fails because the sandbox contains an orphaned process that
    /// does not belong to this run.
    FallbackWithADecoy,
    /// [`pkill_signals_a_child`] fails under [`Confinement::Sandbox`] when its
    /// parent is in its sandbox.
    ParentInTheSandbox,
    /// [`pkill_signals_a_child`] fails without [`CONFINEMENT`].
    NoConfinement,
    /// [`pkill_signals_a_child`] fails with an unknown [`CONFINEMENT`].
    UnknownConfinement,
}

impl Scenario {
    const ALL: [Self; 6] = [
        Self::Fallback,
        Self::FallbackWithAnOrphan,
        Self::FallbackWithADecoy,
        Self::ParentInTheSandbox,
        Self::NoConfinement,
        Self::UnknownConfinement,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Fallback => "fallback",
            Self::FallbackWithAnOrphan => "fallback-with-an-orphan",
            Self::FallbackWithADecoy => "fallback-with-a-decoy",
            Self::ParentInTheSandbox => "parent-in-the-sandbox",
            Self::NoConfinement => "no-confinement",
            Self::UnknownConfinement => "unknown-confinement",
        }
    }
}

/// A value of [`SCENARIO`] that is not the name of a [`Scenario`].
#[derive(Debug, thiserror::Error)]
#[error("{SCENARIO} is {0:?}, which is not one of {names:?}", names = Scenario::ALL.map(Scenario::name))]
struct UnknownScenario(String);

impl std::str::FromStr for Scenario {
    type Err = UnknownScenario;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.name() == name)
            .ok_or_else(|| UnknownScenario(name.to_owned()))
    }
}

// Each scenario runs in a sandbox that lets a process signal only the
// processes in its own sandbox, so no signal from a scenario reaches a process
// outside it, even if a check is wrong. Each run of `sandbox-exec` creates a
// sandbox of its own, so the scenarios cannot signal each other's processes
// either. macOS refuses to apply `SIGNALS_STAY_IN_THE_SANDBOX` under
// `SIGNALS_STAY_IN_THE_OUTER_SANDBOX`, so the fallback scenarios take the
// fallback, and their decoy and orphan are in the outer sandbox. The other
// scenarios run `pkill_signals_a_child` under `SIGNALS_STAY_IN_THE_SANDBOX`.
//
// In a Nix build, the test binary already runs in Nix's sandbox, and macOS
// refuses to apply either profile there. When `nix/checks.nix` has set
// `NIX_BUILD`, the test then writes to standard error that the scenario did not
// run, and passes. The scenarios therefore run only outside Nix, for example in
// `just test`. Elsewhere, a refusal fails the test.
#[rstest]
#[case::fallback(SIGNALS_STAY_IN_THE_OUTER_SANDBOX, Scenario::Fallback)]
#[case::fallback_with_an_orphan(SIGNALS_STAY_IN_THE_OUTER_SANDBOX, Scenario::FallbackWithAnOrphan)]
#[case::fallback_with_a_decoy(SIGNALS_STAY_IN_THE_OUTER_SANDBOX, Scenario::FallbackWithADecoy)]
#[case::parent_in_the_sandbox(SIGNALS_STAY_IN_THE_SANDBOX, Scenario::ParentInTheSandbox)]
#[case::no_confinement(SIGNALS_STAY_IN_THE_SANDBOX, Scenario::NoConfinement)]
#[case::unknown_confinement(SIGNALS_STAY_IN_THE_SANDBOX, Scenario::UnknownConfinement)]
fn a_scenario_in_a_sandbox_passes(
    #[case] profile: &str,
    #[case] scenario: Scenario,
) -> io::Result<()> {
    let output = Command::new("/usr/bin/sandbox-exec")
        .args(["-p", profile])
        .arg(std::env::current_exe()?)
        .args(["--ignored", "--exact", "--quiet", "run_scenario"])
        .env(SCENARIO, scenario.name())
        .env_remove(MARKER)
        .env_remove(CONFINEMENT)
        .output()?;

    if refused(&output) && std::env::var_os(NIX_BUILD).is_some() {
        io::stderr().write_all(
            format!(
                "the scenario {scenario:?} did not run, because macOS refuses to apply its \
                 sandbox in the sandbox of this test\n"
            )
            .as_bytes(),
        )?;
        return Ok(());
    }

    assert_eq!(TestRun::from(&output), TestRun::Passed);

    Ok(())
}

/// Runs the [`Scenario`] that [`SCENARIO`] names, as a child of
/// [`a_scenario_in_a_sandbox_passes`].
#[test]
#[ignore = "a_scenario_in_a_sandbox_passes runs it as a child"]
fn run_scenario() -> Result<(), Box<dyn std::error::Error>> {
    let name = match std::env::var(SCENARIO) {
        Err(std::env::VarError::NotPresent) => return Ok(()),
        name => name?,
    };

    match name.parse()? {
        Scenario::Fallback => {
            assert_matches!(
                confined_pkill_test(SIGNALS_STAY_IN_THE_SANDBOX),
                Ok(Confinement::CheckedProcessTable)
            );
        }
        Scenario::FallbackWithAnOrphan => {
            let orphan = Orphan::leave(Role::LeaveAnOrphan)?;

            assert_matches!(
                (
                    confined_pkill_test(SIGNALS_STAY_IN_THE_SANDBOX),
                    probe(orphan.pid)
                ),
                (Ok(Confinement::CheckedProcessTable), Ok(()))
            );
        }
        Scenario::FallbackWithADecoy => {
            let decoy = Orphan::leave(Role::LeaveADecoy)?;

            assert_matches!(
                confined_pkill_test(SIGNALS_STAY_IN_THE_SANDBOX),
                Err(PkillTestError::Unconfined(pids)) if pids == [decoy.pid]
            );
        }
        Scenario::ParentInTheSandbox => fails_with(
            Some(Confinement::Sandbox.name()),
            "assertion failed: `Ok(())` does not match",
        )?,
        Scenario::NoConfinement => fails_with(None, "Error: NotPresent")?,
        Scenario::UnknownConfinement => {
            fails_with(Some("Sandbox"), "Error: UnknownConfinement(\"Sandbox\")")?;
        }
    }

    Ok(())
}

/// Checks that [`pkill_signals_a_child`] fails with `error` before it runs
/// pkill, when it runs as a child of this process with `confinement`.
fn fails_with(confinement: Option<&str>, error: &str) -> io::Result<()> {
    let run = TestRun::from(&run_pkill_test(confinement, None)?);

    assert_matches!(
        &run,
        TestRun::Failed { stdout, .. } if stdout.contains(error),
        "expected the pkill test to fail with {error:?}"
    );

    Ok(())
}

/// A process whose parent has exited, so that it is not a descendant of this
/// process. It runs [`child`] and exits soon after the value is dropped, once
/// its standard input closes.
struct Orphan {
    pid: Pid,
    /// The write end of the orphan's standard input, which the orphan
    /// inherited from its parent.
    _input: std::process::ChildStdin,
}

impl Orphan {
    /// Starts a child in `role`, which leaves the orphan.
    fn leave(role: Role) -> Result<Self, Box<dyn std::error::Error>> {
        let mut parent = Child::start(role, |_| {})?;
        let pid = Pid::from(parent.report.parse::<i32>()?);
        // `wait` closes the standard input of the child that it waits for.
        let input = parent
            .process
            .stdin
            .take()
            .ok_or("the orphan's parent has no pipe for its standard input")?;
        parent.wait_for_end(END_LIMIT)?;

        Ok(Self { pid, _input: input })
    }
}

/// Starts [`child`] with `marker` and with this process's standard input,
/// reports the child's pid once the child is ready, and returns without
/// waiting for the child.
fn leave_a_child(marker: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut orphan = Command::new(std::env::current_exe()?)
        .args([
            "--ignored",
            "--exact",
            "--quiet",
            Role::Plain.test(),
            marker,
        ])
        .env(MARKER, marker)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = orphan
        .stdout
        .take()
        .ok_or("the orphan has no pipe for its standard output")?;
    let ready = ready_report(&mut BufReader::new(stdout), Instant::now() + READY_LIMIT)?;

    if !matches!(ready, Ready::Report(_)) {
        let _ = orphan.kill();
        return Err(format!("the orphan was not ready: {ready:?}").into());
    }

    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{READY} {}", orphan.id())?;
    stdout.flush()?;

    Ok(())
}

/// Runs as a child that leaves an orphan with a marker of this run.
#[test]
#[ignore = "the scenarios run it as a child"]
fn leave_an_orphan() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os(MARKER).is_none() {
        return Ok(());
    }

    leave_a_child(&unique_marker())
}

/// Runs as a child that leaves an orphan with a marker of no run.
#[test]
#[ignore = "the scenarios run it as a child"]
fn leave_a_decoy() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os(MARKER).is_none() {
        return Ok(());
    }

    leave_a_child("procps-darwin-decoy")
}

/// Runs as a child of [`confined_pkill_test`]. In a sandbox, the test process
/// that started the sandbox is outside it, so signalling that process must fail
/// before pkill runs. The test fails before it runs pkill if [`CONFINEMENT`] is
/// not set to the name of a [`Confinement`].
#[test]
#[ignore = "pkill_signals_the_selected_child runs it as a child"]
fn pkill_signals_a_child() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os(MARKER).is_none() {
        return Ok(());
    }

    match std::env::var(CONFINEMENT)?.parse()? {
        Confinement::Sandbox => {
            let outside = Pid::from(i32::try_from(std::os::unix::process::parent_id())?);
            assert_matches!(
                outside.send_signal(0),
                Err(darwin_proc::Error::Denied { pid, call: Call::Signal }) if pid == outside
            );
        }
        Confinement::CheckedProcessTable => {}
    }

    let mut child = Child::new()?;
    let pid = child.pid()?;
    let name = std::env::current_exe()?
        .file_name()
        .ok_or("the test binary has no file name")?
        .to_string_lossy()
        .into_owned();
    let comm: String = name.chars().take(15).collect();

    let outcome = run(
        env!("CARGO_BIN_EXE_pkill"),
        &["-KILL", "-e", "-f", &child.pattern()],
    )?;

    assert_eq!(
        outcome,
        (
            format!("{comm} killed (pid {pid})\n"),
            String::new(),
            Some(0)
        )
    );
    assert_eq!(child.wait_for_end(END_LIMIT)?.signal(), Some(libc::SIGKILL));

    Ok(())
}

// The tools measure a process's age from the boot time, so no process is older
// than the uptime, even after the clock is set. launchd has run since boot, so
// it is older than a second without the test waiting. The child starts after
// the test reads the uptime, so the child is younger than that uptime unless
// the tools read its start time wrongly.
#[test]
fn a_process_is_selected_only_when_older_than_the_limit() -> Result<(), Box<dyn std::error::Error>>
{
    let uptime = Host::uptime()?.as_secs();
    let child = Child::new()?;

    assert_eq!(
        (
            pgrep(&["-p", "1", "-O", "1"])?,
            pgrep(&["-p", "1", "-O", &(uptime + 3600).to_string()])?,
            pgrep(&["-f", &child.pattern()])?,
            pgrep(&["-O", &uptime.max(1).to_string(), "-f", &child.pattern()])?,
        ),
        (
            selected(&[Pid::from(1)]),
            selected(&[]),
            selected(&[child.pid()?]),
            selected(&[])
        )
    );

    Ok(())
}
