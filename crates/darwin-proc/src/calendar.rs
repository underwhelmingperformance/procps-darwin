// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{ffi::CStr, fmt, sync::Once};

use crate::ffi;

/// A time broken down into its calendar fields, in the local time zone or in
/// UTC, as `localtime_r` and `gmtime_r` give it.
///
/// ```
/// use darwin_proc::CalendarTime;
///
/// let time = CalendarTime::utc(86_400 * 365).expect("1971 is a valid time");
///
/// assert_eq!((time.year(), time.day_of_year()), (1971, 0));
/// ```
#[derive(Clone, Copy)]
pub struct CalendarTime(libc::tm);

impl CalendarTime {
    /// `seconds` since the Unix epoch in the local time zone, which the `TZ`
    /// variable sets, or `None` if `localtime_r` cannot convert them.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// assert!(CalendarTime::local(0).is_some());
    /// ```
    #[must_use]
    pub fn local(seconds: i64) -> Option<Self> {
        static TIME_ZONE: Once = Once::new();

        // POSIX does not require `localtime_r` to read `TZ`, so read it
        // first.
        TIME_ZONE.call_once(|| {
            // SAFETY: `tzset` takes no arguments. It reads `TZ` and the time
            // zone files, and writes the C library's time zone state, such as
            // `tzname`, under the C library's own lock. Reading the
            // environment is sound while no other thread changes it, and the
            // tools do not change their environment.
            unsafe { ffi::tzset() };
        });

        Self::convert(seconds, libc::localtime_r)
    }

    /// `seconds` since the Unix epoch in UTC, or `None` if `gmtime_r` cannot
    /// convert them.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// let time = CalendarTime::utc(3_661).expect("1970 is a valid time");
    ///
    /// assert_eq!((time.hour(), time.minute(), time.second()), (1, 1, 1));
    /// ```
    #[must_use]
    pub fn utc(seconds: i64) -> Option<Self> {
        Self::convert(seconds, libc::gmtime_r)
    }

    /// Converts `seconds` with `convert`, which is `localtime_r` or
    /// `gmtime_r`.
    fn convert(
        seconds: i64,
        convert: unsafe extern "C" fn(*const libc::time_t, *mut libc::tm) -> *mut libc::tm,
    ) -> Option<Self> {
        let seconds: libc::time_t = seconds;
        // SAFETY: `tm` is plain data, and every field may be zero.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };

        // SAFETY: both pointers refer to valid values for the call, which
        // writes only to `tm`.
        let result = unsafe { convert(&raw const seconds, &raw mut tm) };

        (!result.is_null()).then_some(Self(tm))
    }

    /// The year, such as 2026.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// assert_eq!(CalendarTime::utc(0).map(|time| time.year()), Some(1970));
    /// ```
    #[must_use]
    pub const fn year(&self) -> i32 {
        self.0.tm_year + 1900
    }

    /// The month, from 0 for January to 11 for December.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// assert_eq!(CalendarTime::utc(0).map(|time| time.month()), Some(0));
    /// ```
    #[must_use]
    pub const fn month(&self) -> i32 {
        self.0.tm_mon
    }

    /// The day of the month, from 1.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// assert_eq!(
    ///     CalendarTime::utc(0).map(|time| time.day_of_month()),
    ///     Some(1)
    /// );
    /// ```
    #[must_use]
    pub const fn day_of_month(&self) -> i32 {
        self.0.tm_mday
    }

    /// The day of the year, from 0 for 1 January.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// assert_eq!(CalendarTime::utc(0).map(|time| time.day_of_year()), Some(0));
    /// ```
    #[must_use]
    pub const fn day_of_year(&self) -> i32 {
        self.0.tm_yday
    }

    /// The day of the week, from 0 for Sunday.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// // 1 January 1970 was a Thursday.
    /// assert_eq!(CalendarTime::utc(0).map(|time| time.day_of_week()), Some(4));
    /// ```
    #[must_use]
    pub const fn day_of_week(&self) -> i32 {
        self.0.tm_wday
    }

    /// The hour, from 0 to 23.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// assert_eq!(CalendarTime::utc(7_200).map(|time| time.hour()), Some(2));
    /// ```
    #[must_use]
    pub const fn hour(&self) -> i32 {
        self.0.tm_hour
    }

    /// The minute, from 0 to 59.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// assert_eq!(CalendarTime::utc(120).map(|time| time.minute()), Some(2));
    /// ```
    #[must_use]
    pub const fn minute(&self) -> i32 {
        self.0.tm_min
    }

    /// The second, from 0 to 60.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// assert_eq!(CalendarTime::utc(2).map(|time| time.second()), Some(2));
    /// ```
    #[must_use]
    pub const fn second(&self) -> i32 {
        self.0.tm_sec
    }

    /// The time formatted by `strftime` with `format`, in the calling
    /// thread's locale, or `None` if the result is empty or needs `capacity`
    /// bytes or more. `strftime` needs a byte for the terminating NUL, so a
    /// result has at most `capacity - 1` bytes.
    ///
    /// ```
    /// use darwin_proc::CalendarTime;
    ///
    /// let time = CalendarTime::utc(0).expect("1970 is a valid time");
    ///
    /// assert_eq!(
    ///     (time.format(c"%a %b %e %Y", 64), time.format(c"%Y", 4)),
    ///     (Some(b"Thu Jan  1 1970".to_vec()), None)
    /// );
    /// ```
    #[must_use]
    pub fn format(&self, format: &CStr, capacity: usize) -> Option<Vec<u8>> {
        let mut buffer = vec![0_u8; capacity];

        // SAFETY: `buffer` has `capacity` writable bytes, `format` is a
        // NUL-terminated string, and `self.0` is a `tm` that `localtime_r` or
        // `gmtime_r` filled. `strftime` writes at most `capacity` bytes.
        let length = unsafe {
            libc::strftime(
                buffer.as_mut_ptr().cast(),
                capacity,
                format.as_ptr(),
                &raw const self.0,
            )
        };

        if length == 0 {
            return None;
        }

        buffer.truncate(length);
        Some(buffer)
    }
}

impl fmt::Debug for CalendarTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CalendarTime")
            .field("year", &self.year())
            .field("month", &self.month())
            .field("day_of_month", &self.day_of_month())
            .field("hour", &self.hour())
            .field("minute", &self.minute())
            .field("second", &self.second())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::CalendarTime;

    #[rstest]
    #[case::epoch(0, c"%Y-%m-%d %H:%M:%S %j %w", "1970-01-01 00:00:00 001 4")]
    #[case::leap_day(951_782_400, c"%Y-%m-%d %H:%M:%S %j %w", "2000-02-29 00:00:00 060 2")]
    #[case::before_the_epoch(-1, c"%Y-%m-%d %H:%M:%S", "1969-12-31 23:59:59")]
    fn a_time_is_broken_down_and_formatted(
        #[case] seconds: i64,
        #[case] format: &std::ffi::CStr,
        #[case] expected: &str,
    ) {
        let formatted = CalendarTime::utc(seconds).and_then(|time| time.format(format, 64));

        assert_eq!(formatted, Some(expected.as_bytes().to_vec()));
    }

    #[test]
    fn the_fields_follow_struct_tm() {
        let time = CalendarTime::utc(951_782_400 + 3_723);
        let fields = time.map(|time| {
            (
                time.year(),
                time.month(),
                time.day_of_month(),
                time.day_of_year(),
                time.day_of_week(),
                time.hour(),
                time.minute(),
                time.second(),
            )
        });

        assert_eq!(fields, Some((2000, 1, 29, 59, 2, 1, 2, 3)));
    }

    #[test]
    fn an_empty_result_is_none() {
        assert_eq!(
            CalendarTime::utc(0).and_then(|time| time.format(c"", 64)),
            None
        );
    }
}
