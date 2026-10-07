// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::time::Instant;

use crate::SourceError;

/// Limits on the time and memory that one snapshot may use.
///
/// A source that reaches a limit stops reading and returns
/// [`SourceError::TimedOut`] or [`SourceError::TooLarge`]. The memory limit
/// applies to an estimate of the snapshot's size, which the source updates
/// after each process and after each memory region. Only [`LocalSource`]
/// applies a budget. The default [`ProcessSource::snapshot_within`] ignores
/// it.
///
/// [`LocalSource`]: crate::LocalSource
/// [`ProcessSource::snapshot_within`]: crate::ProcessSource::snapshot_within
///
/// ```
/// use std::time::{Duration, Instant};
///
/// use procps_core::Budget;
///
/// let budget = Budget::UNLIMITED
///     .until(Instant::now() + Duration::from_secs(10))
///     .bytes(1 << 20);
///
/// assert_ne!(budget, Budget::UNLIMITED);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Budget {
    until: Option<Instant>,
    bytes: Option<usize>,
}

impl Budget {
    /// A budget with no limits.
    ///
    /// ```
    /// use procps_core::Budget;
    ///
    /// assert_eq!(Budget::UNLIMITED, Budget::UNLIMITED.clone());
    /// ```
    pub const UNLIMITED: Self = Self {
        until: None,
        bytes: None,
    };

    /// Returns this budget with the deadline `until`.
    ///
    /// ```
    /// use std::time::Instant;
    ///
    /// use procps_core::Budget;
    ///
    /// assert_ne!(Budget::UNLIMITED.until(Instant::now()), Budget::UNLIMITED);
    /// ```
    #[must_use]
    pub const fn until(self, until: Instant) -> Self {
        Self {
            until: Some(until),
            ..self
        }
    }

    /// Returns this budget with a limit of `bytes` on the snapshot's estimated
    /// size.
    ///
    /// ```
    /// use procps_core::Budget;
    ///
    /// assert_ne!(Budget::UNLIMITED.bytes(1), Budget::UNLIMITED);
    /// ```
    #[must_use]
    pub const fn bytes(self, bytes: usize) -> Self {
        Self {
            bytes: Some(bytes),
            ..self
        }
    }
}

/// A [`Budget`] and the memory spent from it so far.
#[derive(Debug)]
pub(crate) struct Spending {
    budget: Budget,
    bytes: usize,
}

impl Spending {
    /// Starts with nothing spent from `budget`.
    pub(crate) const fn new(budget: Budget) -> Self {
        Self { budget, bytes: 0 }
    }

    /// Adds `bytes` to the memory used, and checks both limits.
    pub(crate) fn spend(&mut self, bytes: usize) -> Result<(), SourceError> {
        self.bytes = self.bytes.saturating_add(bytes);

        if let Some(limit) = self.budget.bytes
            && self.bytes > limit
        {
            return Err(SourceError::TooLarge { limit });
        }

        if let Some(until) = self.budget.until
            && Instant::now() >= until
        {
            return Err(SourceError::TimedOut);
        }

        Ok(())
    }
}
