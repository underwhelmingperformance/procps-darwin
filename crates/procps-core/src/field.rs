// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

/// A value that a source read for a process, or the reason why the source has
/// no value.
///
/// The formatters decide how to show each case. Most columns show `-` whatever
/// the reason, but some can show the cases differently. procps-ng shows a
/// process without arguments, such as a kernel thread, as `[comm]`. A formatter
/// can show `[comm]` for `kernel_task`, whose arguments are unsupported, and
/// `-` for a user process whose arguments are denied.
///
/// ```
/// use procps_core::Field;
///
/// let size = Field::Available(4096);
///
/// assert_eq!(size.map(|bytes| bytes / 1024), Field::Available(4));
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Field<T> {
    /// The source read the value.
    Available(T),
    /// The caller may not read the value for this process.
    Denied,
    /// macOS does not provide the value for this process, for example the
    /// arguments of `kernel_task`.
    Unsupported,
    /// The read failed for another reason, such as a malformed reply from the
    /// kernel. The source logs the error.
    Failed,
}

impl<T> Field<T> {
    /// The value, if the source read it.
    ///
    /// ```
    /// use procps_core::Field;
    ///
    /// assert_eq!(
    ///     (
    ///         Field::Available(1).available(),
    ///         Field::<i32>::Denied.available()
    ///     ),
    ///     (Some(&1), None)
    /// );
    /// ```
    #[must_use]
    pub const fn available(&self) -> Option<&T> {
        match self {
            Self::Available(value) => Some(value),
            Self::Denied | Self::Unsupported | Self::Failed => None,
        }
    }

    /// Applies `f` to an available value, and returns any other variant
    /// unchanged.
    ///
    /// ```
    /// use procps_core::Field;
    ///
    /// assert_eq!(Field::<i32>::Denied.map(|value| value + 1), Field::Denied);
    /// ```
    #[must_use]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Field<U> {
        match self {
            Self::Available(value) => Field::Available(f(value)),
            Self::Denied => Field::Denied,
            Self::Unsupported => Field::Unsupported,
            Self::Failed => Field::Failed,
        }
    }
}
