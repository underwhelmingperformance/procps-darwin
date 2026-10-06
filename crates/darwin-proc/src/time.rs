// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{io, sync::OnceLock, time::Duration};

use crate::ffi;

/// The ratio that converts Mach absolute time to nanoseconds.
///
/// `proc_taskinfo` and `rusage_info` report CPU time in Mach absolute time.
/// On Apple silicon one unit is 125/3 nanoseconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Timebase {
    numer: u32,
    denom: u32,
}

impl Timebase {
    pub(crate) const fn new(numer: u32, denom: u32) -> Self {
        Self { numer, denom }
    }

    /// This machine's timebase. It does not change, so it is read once and
    /// cached.
    pub(crate) fn current() -> io::Result<Self> {
        static CURRENT: OnceLock<Option<Timebase>> = OnceLock::new();

        let current = CURRENT.get_or_init(|| {
            let mut info = ffi::MachTimebaseInfo { numer: 0, denom: 0 };

            // SAFETY: `info` is a valid `mach_timebase_info` for the call to
            // fill.
            let result = unsafe { ffi::mach_timebase_info(&raw mut info) };

            (result == 0 && info.denom != 0).then(|| Self::new(info.numer, info.denom))
        });

        current.ok_or_else(|| io::Error::other("mach_timebase_info failed"))
    }

    /// The duration of `ticks` units of Mach absolute time.
    pub(crate) fn duration(self, ticks: u64) -> Duration {
        let nanoseconds =
            u128::from(ticks) * u128::from(self.numer) / u128::from(self.denom.max(1));

        Duration::from_nanos(u64::try_from(nanoseconds).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::Timebase;

    #[rstest]
    #[case::apple_silicon(Timebase::new(125, 3), 24_000_000, Duration::from_secs(1))]
    #[case::overflow_saturates(Timebase::new(125, 3), u64::MAX, Duration::from_nanos(u64::MAX))]
    fn ticks_become_a_duration(
        #[case] timebase: Timebase,
        #[case] ticks: u64,
        #[case] expected: Duration,
    ) {
        assert_eq!(timebase.duration(ticks), expected);
    }
}
