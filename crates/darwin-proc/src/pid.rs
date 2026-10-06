// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

/// A process ID. Darwin's `kernel_task` has process ID 0.
///
/// ```
/// use darwin_proc::Pid;
///
/// let launchd = Pid::from(1);
///
/// assert_eq!((launchd.as_raw(), launchd.to_string()), (1, "1".to_owned()));
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
pub struct Pid(libc::pid_t);

impl Pid {
    /// The process ID of the calling process.
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// assert_eq!(Pid::current().as_raw().cast_unsigned(), std::process::id());
    /// ```
    #[must_use]
    pub fn current() -> Self {
        Self(std::process::id().cast_signed())
    }

    /// The process ID as the C type `pid_t`.
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// assert_eq!(Pid::from(42).as_raw(), 42);
    /// ```
    #[must_use]
    pub const fn as_raw(self) -> libc::pid_t {
        self.0
    }
}
