// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use darwin_proc::Uid;

use crate::{
    ErrorChain, LocalSource, ProcessSource, Snapshot, SnapshotRequest, SourceError,
    helper::{HelperSource, SOCKET},
};

/// The environment variable that forces a source: `local` or `helper`.
///
/// ```
/// assert_eq!(procps_core::SOURCE_VARIABLE, "PROCPS_DARWIN_SOURCE");
/// ```
pub const SOURCE_VARIABLE: &str = "PROCPS_DARWIN_SOURCE";

/// The source that a tool reads process data from.
///
/// ```
/// use procps_core::{LocalSource, ProcessSource, SnapshotRequest, Source};
///
/// let source = Source::Local(LocalSource);
///
/// assert!(source.snapshot(&SnapshotRequest::default()).is_ok());
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Source {
    /// Reads process data in the tool itself.
    Local(LocalSource),
    /// Reads process data through the helper, and returns the helper's errors.
    Helper(HelperSource),
    /// Reads process data through the helper, or in the tool itself when the
    /// helper fails.
    HelperOrLocal(HelperOrLocal),
}

impl ProcessSource for Source {
    fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
        match self {
            Self::Local(source) => source.snapshot(request),
            Self::Helper(source) => source.snapshot(request),
            Self::HelperOrLocal(source) => source.snapshot(request),
        }
    }
}

/// Reads process data through the helper, or in the tool itself when the
/// helper fails.
///
/// After an error that the helper would repeat for every request, such as an
/// untrusted socket or another protocol version, the source stops asking the
/// helper. A tool such as `top` then reads every later snapshot itself, so it
/// does not wait for a failing helper on each refresh.
///
/// ```
/// use procps_core::{HelperOrLocal, ProcessSource, SnapshotRequest, helper::HelperSource};
///
/// let source = HelperOrLocal::new(HelperSource::new("/nonexistent/helper.sock"));
/// source.snapshot(&SnapshotRequest::default())?;
///
/// assert!(!source.uses_helper());
/// # Ok::<(), procps_core::SourceError>(())
/// ```
#[derive(Debug)]
pub struct HelperOrLocal {
    helper: HelperSource,
    abandoned: AtomicBool,
}

impl Clone for HelperOrLocal {
    fn clone(&self) -> Self {
        Self {
            helper: self.helper.clone(),
            abandoned: AtomicBool::new(self.abandoned.load(Ordering::Relaxed)),
        }
    }
}

impl PartialEq for HelperOrLocal {
    fn eq(&self, other: &Self) -> bool {
        self.helper == other.helper && self.uses_helper() == other.uses_helper()
    }
}

impl Eq for HelperOrLocal {}

impl HelperOrLocal {
    /// A source that reads through `helper` until the helper fails in a way
    /// that it would repeat.
    ///
    /// ```
    /// use procps_core::{
    ///     HelperOrLocal,
    ///     helper::{HelperSource, SOCKET},
    /// };
    ///
    /// assert!(HelperOrLocal::new(HelperSource::new(SOCKET)).uses_helper());
    /// ```
    #[must_use]
    pub const fn new(helper: HelperSource) -> Self {
        Self {
            helper,
            abandoned: AtomicBool::new(false),
        }
    }

    /// Whether the source still asks the helper for snapshots.
    ///
    /// ```
    /// use procps_core::{
    ///     HelperOrLocal,
    ///     helper::{HelperSource, SOCKET},
    /// };
    ///
    /// assert!(HelperOrLocal::new(HelperSource::new(SOCKET)).uses_helper());
    /// ```
    #[must_use]
    pub fn uses_helper(&self) -> bool {
        !self.abandoned.load(Ordering::Relaxed)
    }
}

impl ProcessSource for HelperOrLocal {
    fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
        if !self.uses_helper() {
            return LocalSource.snapshot(request);
        }

        match self.helper.snapshot(request) {
            Err(SourceError::Helper(error)) if error.is_persistent() => {
                tracing::warn!(
                    error = %ErrorChain(&error),
                    "the helper failed, so reading process data in the tool itself from now on"
                );
                self.abandoned.store(true, Ordering::Relaxed);
                LocalSource.snapshot(request)
            }
            Err(error) => {
                tracing::debug!(
                    error = %ErrorChain(&error),
                    "the helper failed, so reading process data in the tool itself"
                );
                LocalSource.snapshot(request)
            }
            result => result,
        }
    }
}

/// The inputs that decide a tool's source at start-up.
///
/// ```
/// use std::path::PathBuf;
///
/// use darwin_proc::Uid;
/// use procps_core::{Source, SourceChoice};
///
/// let choice = SourceChoice {
///     variable: None,
///     euid: Uid::from(0),
///     socket: PathBuf::from("/nonexistent"),
/// };
///
/// assert!(matches!(choice.choose()?, Source::Local(_)));
/// # Ok::<(), procps_core::ChoiceError>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceChoice {
    /// The value of [`SOURCE_VARIABLE`], if it is set.
    pub variable: Option<OsString>,
    /// The tool's effective user ID.
    pub euid: Uid,
    /// The path of the helper's socket.
    pub socket: PathBuf,
}

/// An error in the choice of a source.
#[derive(Debug, thiserror::Error)]
pub enum ChoiceError {
    /// [`SOURCE_VARIABLE`] has a value other than `local`, `helper` or the
    /// empty string.
    #[error("{SOURCE_VARIABLE} is {0:?}, but it must be \"local\", \"helper\" or empty")]
    Unknown(OsString),
}

impl SourceChoice {
    /// The choice for this process: the value of [`SOURCE_VARIABLE`], the
    /// process's effective user ID and [`SOCKET`].
    ///
    /// ```
    /// use procps_core::{SourceChoice, helper::SOCKET};
    ///
    /// assert_eq!(SourceChoice::current().socket.as_os_str(), SOCKET);
    /// ```
    #[must_use]
    pub fn current() -> Self {
        Self {
            variable: std::env::var_os(SOURCE_VARIABLE),
            euid: Uid::from(rustix::process::geteuid().as_raw()),
            socket: PathBuf::from(SOCKET),
        }
    }

    /// The source to use.
    ///
    /// [`SOURCE_VARIABLE`] forces a source when it is `local` or `helper`, and
    /// an empty value counts as unset. Root reads process data itself. A user
    /// other than root reads through the helper when its socket exists, and
    /// reads process data itself when the helper fails.
    ///
    /// ```
    /// use std::{ffi::OsString, path::PathBuf};
    ///
    /// use darwin_proc::Uid;
    /// use procps_core::{Source, SourceChoice};
    ///
    /// let choice = SourceChoice {
    ///     variable: Some(OsString::from("helper")),
    ///     euid: Uid::from(501),
    ///     socket: PathBuf::from("/nonexistent"),
    /// };
    ///
    /// assert!(matches!(choice.choose()?, Source::Helper(_)));
    /// # Ok::<(), procps_core::ChoiceError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ChoiceError::Unknown`] if [`SOURCE_VARIABLE`] has another
    /// non-empty value.
    pub fn choose(&self) -> Result<Source, ChoiceError> {
        let helper = || HelperSource::new(&self.socket);

        match self.variable.as_deref().filter(|value| !value.is_empty()) {
            Some(value) if value == OsStr::new("local") => Ok(Source::Local(LocalSource)),
            Some(value) if value == OsStr::new("helper") => Ok(Source::Helper(helper())),
            Some(value) => Err(ChoiceError::Unknown(value.to_owned())),
            None if self.euid.as_raw() == 0 => Ok(Source::Local(LocalSource)),
            None if self.socket.exists() => Ok(Source::HelperOrLocal(HelperOrLocal::new(helper()))),
            None => Ok(Source::Local(LocalSource)),
        }
    }
}
