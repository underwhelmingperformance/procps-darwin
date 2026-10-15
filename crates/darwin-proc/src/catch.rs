// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    io,
    mem::MaybeUninit,
    sync::atomic::{AtomicPtr, AtomicU8, Ordering},
};

use crate::{Call, Error};

/// Darwin's signals are numbered from 1 to 31.
const SIGNAL_COUNT: usize = 32;

/// What the handler that [`catch_signals`] installs does when a signal
/// arrives. The handler first blocks every signal, and then writes the
/// response's message on standard error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignalResponse {
    /// Write `message`, then exit with `status` at once, as `_exit(2)` does.
    /// Nothing that the process has buffered is written.
    Exit {
        /// The message.
        message: Vec<u8>,
        /// The exit status.
        status: u8,
    },
    /// Write `message`, then take the signal's default action. If the action
    /// does not end the process, as for a signal that stops it, exit with
    /// status 1 when the process continues.
    Default {
        /// The message.
        message: Vec<u8>,
    },
}

/// The responses, indexed by the signal's number.
type Table = [Option<SignalResponse>; SIGNAL_COUNT];

/// The response table of the last call to [`catch_signals`] that passed its
/// checks, or null if no call did. A call publishes its table before it
/// installs the first handler, and leaves it published if the installation
/// fails. A table is never freed or changed once it is
/// published.
static RESPONSES: AtomicPtr<Table> = AtomicPtr::new(std::ptr::null_mut());

/// No call to [`catch_signals`] has succeeded, and none is running.
const IDLE: u8 = 0;

/// A call to [`catch_signals`] is installing its handlers.
const INSTALLING: u8 = 1;

/// A call to [`catch_signals`] has succeeded.
const INSTALLED: u8 = 2;

/// Whether [`catch_signals`] has installed its handlers.
static STATE: AtomicU8 = AtomicU8::new(IDLE);

/// Installs a handler with the given response for each signal in
/// `responses`. When `responses` has a signal more than once, the last
/// response applies. The other signals keep their dispositions.
///
/// ```
/// use darwin_proc::SignalResponse;
///
/// let quiet_exit = SignalResponse::Exit {
///     message: Vec::new(),
///     status: 0,
/// };
///
/// darwin_proc::catch_signals([(libc::SIGPIPE, quiet_exit)])?;
/// # Ok::<(), darwin_proc::Error>(())
/// ```
///
/// # Errors
///
/// Returns [`Error::Os`] with `EINVAL` for a signal number outside Darwin's
/// range, or for `SIGKILL` or `SIGSTOP`, which no handler can catch. It
/// returns `EBUSY` if an earlier call succeeded or another call is
/// installing its handlers. These errors change nothing.
///
/// If `sigaction(2)` fails, the function restores the actions that the call
/// replaced, and returns the error. The call can then be repeated. A handler
/// that a signal started before its action was restored still runs. It gives
/// the response of this call, or of a later call if that call publishes its
/// table before the handler reads the table.
pub fn catch_signals(
    responses: impl IntoIterator<Item = (libc::c_int, SignalResponse)>,
) -> Result<(), Error> {
    catch_with(responses, &mut Kernel)
}

/// How [`catch_signals`] replaces the action of a signal, so that tests can
/// make `sigaction(2)` fail.
trait Installer {
    /// Installs [`handle`] for `signal`, and returns the action that it
    /// replaced.
    fn install(&mut self, signal: libc::c_int) -> io::Result<libc::sigaction>;

    /// Restores `previous`, which [`Installer::install`] returned, for
    /// `signal`.
    fn restore(&mut self, signal: libc::c_int, previous: &libc::sigaction);
}

/// The kernel, which installs handlers with `sigaction(2)`.
struct Kernel;

impl Installer for Kernel {
    /// Installs [`handle`] for `signal`, with every signal blocked while it
    /// runs and without `SA_RESTART`, as procps-ng's `ps` installs its
    /// handler.
    fn install(&mut self, signal: libc::c_int) -> io::Result<libc::sigaction> {
        let mut mask = MaybeUninit::<libc::sigset_t>::uninit();

        // SAFETY: `sigfillset` initialises the set that `mask` points to.
        unsafe {
            libc::sigfillset(mask.as_mut_ptr());
        }

        let action = libc::sigaction {
            sa_sigaction: handle as extern "C" fn(libc::c_int) as libc::sighandler_t,
            // SAFETY: `sigfillset` initialised the set.
            sa_mask: unsafe { mask.assume_init() },
            sa_flags: 0,
        };
        let mut previous = MaybeUninit::<libc::sigaction>::uninit();

        // SAFETY: `action` is a valid `sigaction` whose handler is `handle`,
        // which only calls async-signal-safe functions, and `previous` has
        // room for the action that `sigaction` replaces.
        let result = unsafe { libc::sigaction(signal, &raw const action, previous.as_mut_ptr()) };

        if result == -1 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: `sigaction` succeeded, so it wrote the previous action.
        Ok(unsafe { previous.assume_init() })
    }

    fn restore(&mut self, signal: libc::c_int, previous: &libc::sigaction) {
        // SAFETY: `previous` is the action that `sigaction` returned for
        // `signal`, so it is valid to install again. The replaced action is
        // not requested, so the null pointer is allowed.
        unsafe {
            libc::sigaction(signal, previous, std::ptr::null_mut());
        }
    }
}

/// The index of `signal` in a [`Table`], or `None` for a number outside
/// Darwin's range.
fn index(signal: libc::c_int) -> Option<usize> {
    usize::try_from(signal)
        .ok()
        .filter(|index| (1..SIGNAL_COUNT).contains(index))
}

/// Unblocks `signal` for the calling thread. A child process that the thread
/// starts afterwards inherits the thread's signal mask.
///
/// ```
/// darwin_proc::unblock_signal(libc::SIGUSR1)?;
/// # Ok::<(), darwin_proc::Error>(())
/// ```
///
/// # Errors
///
/// Returns [`Error::Os`] with `EINVAL` for a signal number outside Darwin's
/// range, and with the error of `pthread_sigmask(3)` if it fails.
pub fn unblock_signal(signal: libc::c_int) -> Result<(), Error> {
    let error = |source| Error::Os {
        call: Call::SignalMask,
        source,
    };

    if index(signal).is_none() {
        return Err(error(io::Error::from_raw_os_error(libc::EINVAL)));
    }

    let mut only = MaybeUninit::<libc::sigset_t>::uninit();

    // SAFETY: `sigemptyset` and `sigaddset` initialise the set that
    // `pthread_sigmask` reads, and the previous mask is not requested, so the
    // null pointer is allowed.
    let result = unsafe {
        libc::sigemptyset(only.as_mut_ptr());
        libc::sigaddset(only.as_mut_ptr(), signal);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, only.as_ptr(), std::ptr::null_mut())
    };

    if result != 0 {
        return Err(error(io::Error::from_raw_os_error(result)));
    }

    Ok(())
}

/// [`catch_signals`], with `installer` to replace each signal's action.
fn catch_with(
    responses: impl IntoIterator<Item = (libc::c_int, SignalResponse)>,
    installer: &mut impl Installer,
) -> Result<(), Error> {
    let error = |source| Error::Os {
        call: Call::SignalAction,
        source,
    };
    let refused = |code| error(io::Error::from_raw_os_error(code));
    let mut table: Table = Default::default();
    let mut signals = Vec::new();

    for (signal, response) in responses {
        let index = index(signal)
            .filter(|_| signal != libc::SIGKILL && signal != libc::SIGSTOP)
            .ok_or_else(|| refused(libc::EINVAL))?;

        if table[index].is_none() {
            signals.push(signal);
        }

        table[index] = Some(response);
    }

    STATE
        .compare_exchange(IDLE, INSTALLING, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| refused(libc::EBUSY))?;

    // The table is leaked and stays published after a failure, because a
    // handler that started on another thread before the rollback can still
    // read it. With the table cleared, that handler would take the default
    // action and replace the action that the rollback restored.
    RESPONSES.store(Box::into_raw(Box::new(table)), Ordering::Release);

    let mut replaced = Vec::new();

    for signal in signals {
        match installer.install(signal) {
            Ok(previous) => replaced.push((signal, previous)),
            Err(source) => {
                for (signal, previous) in replaced.iter().rev() {
                    installer.restore(*signal, previous);
                }

                STATE.store(IDLE, Ordering::Release);
                return Err(error(source));
            }
        }
    }

    STATE.store(INSTALLED, Ordering::Release);
    Ok(())
}

/// Writes all of `bytes` on standard error, and ignores a failure, as
/// procps-ng does in its handler.
fn write_stderr(mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: `bytes` is valid for reads of its length.
        let written =
            unsafe { libc::write(libc::STDERR_FILENO, bytes.as_ptr().cast(), bytes.len()) };

        match usize::try_from(written) {
            Ok(written) if written > 0 => bytes = &bytes[written..],
            _ if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) => {}
            _ => return,
        }
    }
}

/// The handler that [`catch_signals`] installs. It reads the response that
/// [`RESPONSES`] has for `signal`, which was published before any handler was
/// installed, and calls only async-signal-safe functions.
extern "C" fn handle(signal: libc::c_int) {
    let mut every = MaybeUninit::<libc::sigset_t>::uninit();

    // SAFETY: `sigfillset` initialises the set, and `sigprocmask` reads it.
    // The previous mask is not requested, so the null pointer is allowed.
    unsafe {
        libc::sigfillset(every.as_mut_ptr());
        libc::sigprocmask(libc::SIG_BLOCK, every.as_ptr(), std::ptr::null_mut());
    }

    let table = RESPONSES.load(Ordering::Acquire);

    // SAFETY: a non-null pointer refers to a table that `catch_with` leaked
    // and never changes or frees.
    let response = unsafe { table.as_ref() }
        .zip(usize::try_from(signal).ok())
        .and_then(|(table, index)| table.get(index))
        .and_then(Option::as_ref);

    match response {
        Some(SignalResponse::Exit { message, status }) => {
            write_stderr(message);

            // SAFETY: `_exit` ends the process without running any code of
            // the process.
            unsafe { libc::_exit(libc::c_int::from(*status)) }
        }
        Some(SignalResponse::Default { message }) => {
            write_stderr(message);
            take_default_action(signal);
        }
        None => take_default_action(signal),
    }
}

/// Restores the default action of `signal`, unblocks `signal` and sends it to
/// this process, and exits with status 1 if the action does not end the
/// process.
fn take_default_action(signal: libc::c_int) {
    let mut only = MaybeUninit::<libc::sigset_t>::uninit();

    // SAFETY: `SIG_DFL` installs no handler. `sigemptyset` and `sigaddset`
    // initialise the set that `sigprocmask` reads, and the previous mask is
    // not requested. `kill` and `getpid` take no pointers, and `_exit` ends
    // the process without running any code of the process.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::sigemptyset(only.as_mut_ptr());
        libc::sigaddset(only.as_mut_ptr(), signal);
        libc::sigprocmask(libc::SIG_UNBLOCK, only.as_ptr(), std::ptr::null_mut());
        libc::kill(libc::getpid(), signal);
        libc::_exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Mutex, MutexGuard, PoisonError, atomic::Ordering},
    };

    use pretty_assertions::assert_eq;

    use super::{IDLE, INSTALLED, Installer, RESPONSES, STATE, SignalResponse, catch_with};
    use crate::{Call, Error};

    /// A call that [`Fake`] receives.
    #[derive(Debug, Eq, PartialEq)]
    enum Step {
        /// An installation for the signal.
        Install(libc::c_int),
        /// A restoration of the action, identified by its handler, for the
        /// signal.
        Restore(libc::c_int, libc::sighandler_t),
    }

    /// An installer that records its calls and fails for one signal. The
    /// previous action of each signal has the signal's number as its handler.
    struct Fake {
        failing: Option<libc::c_int>,
        steps: Vec<Step>,
    }

    impl Installer for Fake {
        fn install(&mut self, signal: libc::c_int) -> io::Result<libc::sigaction> {
            self.steps.push(Step::Install(signal));

            if self.failing == Some(signal) {
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }

            Ok(libc::sigaction {
                sa_sigaction: usize::try_from(signal).unwrap_or_default(),
                sa_mask: 0,
                sa_flags: 0,
            })
        }

        fn restore(&mut self, signal: libc::c_int, previous: &libc::sigaction) {
            self.steps
                .push(Step::Restore(signal, previous.sa_sigaction));
        }
    }

    /// Each test that changes the global state takes this lock first.
    static GLOBALS: Mutex<()> = Mutex::new(());

    /// Takes [`GLOBALS`] and resets the state to what a process starts with.
    fn fresh() -> MutexGuard<'static, ()> {
        let guard = GLOBALS.lock().unwrap_or_else(PoisonError::into_inner);

        STATE.store(IDLE, Ordering::Release);
        RESPONSES.store(std::ptr::null_mut(), Ordering::Release);
        guard
    }

    /// Whether `catch_signals` has succeeded, and the published response for
    /// `signal`.
    fn published(signal: libc::c_int) -> (u8, Option<SignalResponse>) {
        let table = RESPONSES.load(Ordering::Acquire);

        // SAFETY: a non-null pointer refers to a table that `catch_with`
        // leaked and never changes or frees.
        let response = unsafe { table.as_ref() }
            .zip(usize::try_from(signal).ok())
            .and_then(|(table, index)| table[index].clone());

        (STATE.load(Ordering::Acquire), response)
    }

    fn exit(status: u8) -> SignalResponse {
        SignalResponse::Exit {
            message: Vec::new(),
            status,
        }
    }

    #[test]
    fn a_failed_installation_is_undone_and_can_be_retried() {
        let _fresh = fresh();
        let mut failing = Fake {
            failing: Some(libc::SIGUSR2),
            steps: Vec::new(),
        };
        let failed = catch_with(
            [
                (libc::SIGHUP, exit(0)),
                (libc::SIGUSR1, exit(0)),
                (libc::SIGUSR2, exit(0)),
                (libc::SIGTERM, exit(0)),
            ],
            &mut failing,
        )
        .map_err(|error| match error {
            Error::Os { call, source } => (call, source.raw_os_error()),
            _ => (Call::Signal, None),
        });
        let after_failure = published(libc::SIGUSR1);

        let mut working = Fake {
            failing: None,
            steps: Vec::new(),
        };
        let retried = catch_with([(libc::SIGHUP, exit(1))], &mut working).is_ok();

        assert_eq!(
            (
                failed,
                failing.steps,
                after_failure,
                retried,
                working.steps,
                published(libc::SIGHUP)
            ),
            (
                Err((Call::SignalAction, Some(libc::EPERM))),
                vec![
                    Step::Install(libc::SIGHUP),
                    Step::Install(libc::SIGUSR1),
                    Step::Install(libc::SIGUSR2),
                    Step::Restore(libc::SIGUSR1, 30),
                    Step::Restore(libc::SIGHUP, 1),
                ],
                (IDLE, Some(exit(0))),
                true,
                vec![Step::Install(libc::SIGHUP)],
                (INSTALLED, Some(exit(1))),
            )
        );
    }

    #[test]
    fn the_last_response_for_a_signal_applies() {
        let _fresh = fresh();
        let mut installer = Fake {
            failing: None,
            steps: Vec::new(),
        };
        let succeeded = catch_with(
            [(libc::SIGHUP, exit(0)), (libc::SIGHUP, exit(1))],
            &mut installer,
        )
        .is_ok();

        assert_eq!(
            (succeeded, installer.steps, published(libc::SIGHUP)),
            (
                true,
                vec![Step::Install(libc::SIGHUP)],
                (INSTALLED, Some(exit(1)))
            )
        );
    }

    #[test]
    fn an_uncatchable_signal_is_refused_before_anything_is_installed() {
        let mut installer = Fake {
            failing: None,
            steps: Vec::new(),
        };
        let refused: Vec<Option<i32>> = [libc::SIGKILL, libc::SIGSTOP]
            .into_iter()
            .map(|signal| {
                match catch_with([(libc::SIGHUP, exit(0)), (signal, exit(0))], &mut installer) {
                    Err(Error::Os { source, .. }) => source.raw_os_error(),
                    _ => None,
                }
            })
            .collect();

        assert_eq!(
            (refused, installer.steps),
            (vec![Some(libc::EINVAL), Some(libc::EINVAL)], Vec::new())
        );
    }

    #[test]
    fn a_blocked_signal_is_unblocked() {
        let mut only = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        let mut current = std::mem::MaybeUninit::<libc::sigset_t>::uninit();

        // SAFETY: `sigemptyset` and `sigaddset` initialise `only`, which
        // `pthread_sigmask` reads, and the previous mask is not requested.
        unsafe {
            libc::sigemptyset(only.as_mut_ptr());
            libc::sigaddset(only.as_mut_ptr(), libc::SIGUSR2);
            libc::pthread_sigmask(libc::SIG_BLOCK, only.as_ptr(), std::ptr::null_mut());
        }

        let unblocked = super::unblock_signal(libc::SIGUSR2).is_ok();

        // SAFETY: a null new mask leaves the mask unchanged, and
        // `pthread_sigmask` writes the current mask to `current`, which
        // `sigismember` then reads.
        let blocked = unsafe {
            libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), current.as_mut_ptr());
            libc::sigismember(current.as_ptr(), libc::SIGUSR2)
        };
        let refused: Vec<Option<i32>> = [0, 32]
            .into_iter()
            .map(|signal| match super::unblock_signal(signal) {
                Err(Error::Os {
                    call: Call::SignalMask,
                    source,
                }) => source.raw_os_error(),
                _ => None,
            })
            .collect();

        assert_eq!(
            (unblocked, blocked, refused),
            (true, 0, vec![Some(libc::EINVAL), Some(libc::EINVAL)])
        );
    }
}
