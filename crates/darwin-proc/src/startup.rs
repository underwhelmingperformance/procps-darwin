// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

/// How many descriptors, from 0, the record of closed descriptors covers.
const RECORDED: libc::c_int = 256;

/// The descriptors from 0 to 255 that were closed when the process started:
/// bit `n % 64` of word `n / 64` for descriptor `n`.
static CLOSED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Whether the process started with `SIGPIPE` ignored.
static SIGPIPE_IGNORED: AtomicBool = AtomicBool::new(false);

/// Records the state that Rust's runtime replaces before `main` runs. Rust's
/// runtime opens `/dev/null` in place of a closed standard descriptor, and
/// ignores `SIGPIPE`.
extern "C" fn record() {
    for descriptor in 0..RECORDED {
        // SAFETY: `F_GETFD` only reads the descriptor's flags, and fails with
        // `EBADF` for a closed descriptor.
        if unsafe { libc::fcntl(descriptor, libc::F_GETFD) } == -1 {
            let (word, bit) = position(descriptor);
            CLOSED[word].fetch_or(1 << bit, Ordering::Relaxed);
        }
    }

    let mut previous = MaybeUninit::<libc::sigaction>::uninit();

    // SAFETY: with a null new action, `sigaction` only writes the current
    // action into `previous`, which is large enough for it.
    if unsafe { libc::sigaction(libc::SIGPIPE, std::ptr::null(), previous.as_mut_ptr()) } == 0 {
        // SAFETY: `sigaction` succeeded, so it initialised `previous`.
        let previous = unsafe { previous.assume_init() };
        SIGPIPE_IGNORED.store(previous.sa_sigaction == libc::SIG_IGN, Ordering::Relaxed);
    }
}

/// The word and the bit of `descriptor` in a set of the recorded descriptors.
/// `descriptor` is from 0 to 255.
fn position(descriptor: libc::c_int) -> (usize, u32) {
    let index = descriptor.unsigned_abs();

    (usize::try_from(index / 64).unwrap_or(0), index % 64)
}

// SAFETY: dyld calls each function in `__mod_init_func` once, before the C
// `main` and before Rust's runtime starts. It passes `argc`, `argv` and other
// arguments, which a function without parameters ignores under the C calling
// convention.
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static RECORD: extern "C" fn() = record;

/// One of the standard streams.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandardStream {
    /// Standard input, descriptor 0.
    Input,
    /// Standard output, descriptor 1.
    Output,
    /// Standard error, descriptor 2.
    Error,
}

impl StandardStream {
    const ALL: [Self; 3] = [Self::Input, Self::Output, Self::Error];

    /// The stream's file descriptor.
    ///
    /// ```
    /// use darwin_proc::StandardStream;
    ///
    /// assert_eq!(StandardStream::Error.descriptor(), 2);
    /// ```
    #[must_use]
    pub const fn descriptor(self) -> libc::c_int {
        match self {
            Self::Input => 0,
            Self::Output => 1,
            Self::Error => 2,
        }
    }

    const fn bit(self) -> u8 {
        1 << self.descriptor()
    }
}

/// A set of standard streams whose descriptors are closed.
/// [`ClosedStreams::at_start`] returns the streams that were closed when the
/// process started.
///
/// ```
/// use darwin_proc::{ClosedStreams, StandardStream};
///
/// let closed: ClosedStreams = [StandardStream::Error, StandardStream::Output]
///     .into_iter()
///     .collect();
///
/// assert_eq!(
///     (closed.contains(StandardStream::Input), closed.first()),
///     (false, Some(StandardStream::Output))
/// );
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClosedStreams(u8);

impl ClosedStreams {
    /// The standard streams whose descriptors were closed when this process
    /// started.
    ///
    /// ```
    /// let closed = darwin_proc::ClosedStreams::at_start();
    ///
    /// println!("{closed:?}");
    /// ```
    #[must_use]
    pub fn at_start() -> Self {
        let closed = ClosedDescriptors::at_start();

        StandardStream::ALL
            .into_iter()
            .filter(|stream| closed.contains(stream.descriptor()))
            .collect()
    }

    /// Whether the set contains `stream`.
    ///
    /// ```
    /// use darwin_proc::{ClosedStreams, StandardStream};
    ///
    /// assert!(!ClosedStreams::default().contains(StandardStream::Input));
    /// ```
    #[must_use]
    pub const fn contains(self, stream: StandardStream) -> bool {
        self.0 & stream.bit() != 0
    }

    /// The stream in the set with the lowest descriptor, or `None` if the set
    /// is empty.
    ///
    /// ```
    /// use darwin_proc::{ClosedStreams, StandardStream};
    ///
    /// let closed: ClosedStreams = [StandardStream::Error].into_iter().collect();
    ///
    /// assert_eq!(closed.first(), Some(StandardStream::Error));
    /// ```
    #[must_use]
    pub fn first(self) -> Option<StandardStream> {
        StandardStream::ALL
            .into_iter()
            .find(|&stream| self.contains(stream))
    }
}

impl FromIterator<StandardStream> for ClosedStreams {
    fn from_iter<I: IntoIterator<Item = StandardStream>>(streams: I) -> Self {
        Self(
            streams
                .into_iter()
                .fold(0, |bits, stream| bits | stream.bit()),
        )
    }
}

/// A set of the file descriptors from 0 to 255 that are closed.
/// [`ClosedDescriptors::at_start`] returns the descriptors that were closed
/// when the process started.
///
/// ```
/// use darwin_proc::ClosedDescriptors;
///
/// let closed: ClosedDescriptors = [4, 3].into_iter().collect();
///
/// assert_eq!(
///     (closed.contains(3), closed.contains(5), closed.lowest()),
///     (true, false, Some(3))
/// );
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClosedDescriptors([u64; 4]);

impl ClosedDescriptors {
    /// The descriptors from 0 to 255 that were closed when this process
    /// started.
    ///
    /// ```
    /// let closed = darwin_proc::ClosedDescriptors::at_start();
    ///
    /// println!("{closed:?}");
    /// ```
    #[must_use]
    pub fn at_start() -> Self {
        Self(CLOSED.each_ref().map(|word| word.load(Ordering::Relaxed)))
    }

    /// Whether the set contains `descriptor`. The set contains no descriptor
    /// outside 0 to 255.
    ///
    /// ```
    /// use darwin_proc::ClosedDescriptors;
    ///
    /// assert!(!ClosedDescriptors::default().contains(0));
    /// ```
    #[must_use]
    pub fn contains(self, descriptor: libc::c_int) -> bool {
        if !(0..RECORDED).contains(&descriptor) {
            return false;
        }

        let (word, bit) = position(descriptor);

        self.0[word] >> bit & 1 == 1
    }

    /// The lowest descriptor in the set, or `None` if the set is empty.
    ///
    /// ```
    /// use darwin_proc::ClosedDescriptors;
    ///
    /// let closed: ClosedDescriptors = [7].into_iter().collect();
    ///
    /// assert_eq!(closed.lowest(), Some(7));
    /// ```
    #[must_use]
    pub fn lowest(self) -> Option<libc::c_int> {
        (0..RECORDED).find(|&descriptor| self.contains(descriptor))
    }
}

impl FromIterator<libc::c_int> for ClosedDescriptors {
    fn from_iter<I: IntoIterator<Item = libc::c_int>>(descriptors: I) -> Self {
        let mut words = [0; 4];

        for descriptor in descriptors {
            if (0..RECORDED).contains(&descriptor) {
                let (word, bit) = position(descriptor);
                words[word] |= 1 << bit;
            }
        }

        Self(words)
    }
}

/// Restores the `SIGPIPE` disposition that the process started with, and makes
/// every write to a standard output or standard error that was closed at
/// start-up fail, as such a write would fail if Rust's runtime had not
/// replaced the descriptor. Rust's runtime changes both before `main` runs.
///
/// procps-ng's `pgrep` and `pkill` keep the inherited disposition, and
/// procps-ng's tools leave the closed descriptors closed. With `SIGPIPE`
/// inherited as ignored, a write to a closed pipe fails with `EPIPE`, which
/// procps-ng does not report. Otherwise `SIGPIPE` ends the tool, as
/// `pgrep -a . | head` ends procps-ng's `pgrep`. procps-ng's `ps` catches
/// `SIGPIPE` instead, which [`catch_signals`](crate::catch_signals) can do. A
/// standard output or standard error that was closed at start-up becomes
/// `/dev/null` opened for reading only, so that writes to it fail with `EBADF`,
/// as procps-ng's writes to a closed stream fail. procps-ng's epoll descriptor
/// takes the lowest standard descriptor that was closed, and a write to that
/// stream fails with `EINVAL` instead. `procps::output::Output` emulates that
/// failure. Closing the descriptor instead would let the next file that the
/// tool opens take its number and receive the output.
///
/// ```
/// darwin_proc::restore_inherited_state();
/// ```
pub fn restore_inherited_state() {
    let closed = ClosedStreams::at_start();

    for stream in [StandardStream::Output, StandardStream::Error] {
        if closed.contains(stream) {
            make_unwritable(stream.descriptor());
        }
    }

    let disposition = if SIGPIPE_IGNORED.load(Ordering::Relaxed) {
        libc::SIG_IGN
    } else {
        libc::SIG_DFL
    };

    // SAFETY: `SIG_IGN` and `SIG_DFL` install no handler, so no code of ours
    // runs on the signal, and the previous disposition that `signal` returns
    // is not used.
    unsafe {
        libc::signal(libc::SIGPIPE, disposition);
    }
}

/// Replaces `descriptor` with `/dev/null` opened for reading only.
fn make_unwritable(descriptor: libc::c_int) {
    // SAFETY: the path is a NUL-terminated string that outlives the call.
    let null = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };

    if null == -1 {
        return;
    }

    // SAFETY: both descriptors are open. `dup2` replaces `descriptor`, which
    // Rust's runtime opened on `/dev/null`, and `close` releases `null`, which
    // nothing else uses.
    unsafe {
        libc::dup2(null, descriptor);
        libc::close(null);
    }
}
