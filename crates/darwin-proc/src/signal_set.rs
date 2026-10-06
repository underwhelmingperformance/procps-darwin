// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

/// A set of signals, as the kernel stores it: bit `n - 1` is set for signal
/// `n`.
///
/// ```
/// use darwin_proc::SignalSet;
///
/// let set = SignalSet::from(1 << (libc::SIGTERM - 1));
///
/// assert_eq!(
///     (set.contains(libc::SIGTERM), set.contains(libc::SIGINT)),
///     (true, false)
/// );
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, derive_more::From)]
pub struct SignalSet(u32);

impl SignalSet {
    /// Whether the set contains `signal`.
    ///
    /// ```
    /// use darwin_proc::SignalSet;
    ///
    /// assert!(!SignalSet::default().contains(libc::SIGHUP));
    /// ```
    #[must_use]
    pub fn contains(self, signal: libc::c_int) -> bool {
        let Some(bit) = signal
            .checked_sub(1)
            .and_then(|bit| u32::try_from(bit).ok())
        else {
            return false;
        };

        self.0.checked_shr(bit).is_some_and(|bits| bits & 1 == 1)
    }

    /// The set as a bit mask, as procps-ng prints it in the `caught` and
    /// `ignored` columns.
    ///
    /// ```
    /// use darwin_proc::SignalSet;
    ///
    /// assert_eq!(SignalSet::from(0x4002).bits(), 0x4002);
    /// ```
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::SignalSet;

    #[rstest]
    #[case::first(1, true)]
    #[case::last(32, true)]
    #[case::absent(2, false)]
    #[case::zero(0, false)]
    #[case::negative(-1, false)]
    #[case::beyond_the_mask(33, false)]
    fn membership_follows_the_kernel_bit_layout(#[case] signal: i32, #[case] expected: bool) {
        assert_eq!(SignalSet::from(0x8000_0001).contains(signal), expected);
    }
}
