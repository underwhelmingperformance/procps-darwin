// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{iter::Sum, ops::Add};

use darwin_proc::{Memory, ProcessorTicks, Swap};

/// The figures on `top`'s memory line, in bytes, computed as procps-ng
/// computes them from Linux's `/proc/meminfo`.
///
/// ```
/// use darwin_proc::Host;
/// use procps_core::LinuxMemory;
///
/// let memory = LinuxMemory::from(&Host::memory()?);
///
/// assert_eq!(memory.used + memory.available, memory.total);
/// # Ok::<(), darwin_proc::Error>(())
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinuxMemory {
    /// The installed memory, for Linux's `MemTotal`.
    pub total: u64,
    /// Free memory, for Linux's `MemFree`.
    pub free: u64,
    /// The total minus the available memory.
    pub used: u64,
    /// File-backed and purgeable memory, for Linux's buffers, page cache and
    /// reclaimable slab memory.
    pub buff_cache: u64,
    /// Free, file-backed and purgeable memory, for Linux's `MemAvailable`, or
    /// the free memory if that sum is 0 or larger than the total. The
    /// file-backed memory includes the speculative memory.
    pub available: u64,
}

impl From<&Memory> for LinuxMemory {
    /// Maps Darwin's page categories onto Linux's memory figures.
    ///
    /// procps-ng uses the free memory as the available memory when Linux's
    /// `MemAvailable` is 0 or larger than `MemTotal`. This conversion applies
    /// the same rule to the sum of the free, file-backed and purgeable memory.
    fn from(memory: &Memory) -> Self {
        let buff_cache = memory.file_backed + memory.purgeable;
        let reported = memory.free + buff_cache;
        let available = if reported == 0 || reported > memory.total {
            memory.free
        } else {
            reported
        };

        Self {
            total: memory.total,
            free: memory.free,
            used: memory.total.saturating_sub(available),
            buff_cache,
            available,
        }
    }
}

/// The figures on `top`'s swap line, in bytes.
///
/// ```
/// use darwin_proc::Host;
/// use procps_core::LinuxSwap;
///
/// let swap = LinuxSwap::from(&Host::swap()?);
///
/// assert_eq!(swap.used + swap.free, swap.total);
/// # Ok::<(), darwin_proc::Error>(())
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinuxSwap {
    /// The size of the swap files, for Linux's `SwapTotal`.
    pub total: u64,
    /// The part of the swap files not in use, for Linux's `SwapFree`.
    pub free: u64,
    /// The part of the swap files in use.
    pub used: u64,
}

impl From<&Swap> for LinuxSwap {
    fn from(swap: &Swap) -> Self {
        Self {
            total: swap.total,
            free: swap.available,
            used: swap.total.saturating_sub(swap.available),
        }
    }
}

/// Processor time in scheduler ticks, in the states of `top`'s CPU line.
///
/// Darwin reports only the user, system, idle and nice states, so the other
/// fields are always 0. It also always reports 0 nice ticks.
///
/// ```
/// use darwin_proc::ProcessorTicks;
/// use procps_core::LinuxCpu;
///
/// let before = ProcessorTicks {
///     user: 1,
///     system: 1,
///     idle: 1,
///     nice: 0,
/// };
/// let after = ProcessorTicks {
///     user: 3,
///     system: 2,
///     idle: 7,
///     nice: 0,
/// };
///
/// assert_eq!(LinuxCpu::between(&before, &after).total(), 9);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinuxCpu {
    /// Ticks running user code, `us`.
    pub user: u64,
    /// Ticks running niced user code, `ni`.
    pub nice: u64,
    /// Ticks running kernel code, `sy`.
    pub system: u64,
    /// Idle ticks, `id`.
    pub idle: u64,
    /// Ticks waiting for I/O, `wa`.
    pub iowait: u64,
    /// Ticks handling hardware interrupts, `hi`.
    pub irq: u64,
    /// Ticks handling software interrupts, `si`.
    pub softirq: u64,
    /// Ticks taken by a hypervisor, `st`.
    pub steal: u64,
}

impl LinuxCpu {
    /// The ticks that a processor spent in each state between two readings.
    /// The kernel's counters are 32 bits wide and wrap, so each difference is
    /// taken modulo 2³².
    ///
    /// ```
    /// use darwin_proc::ProcessorTicks;
    /// use procps_core::LinuxCpu;
    ///
    /// let before = ProcessorTicks {
    ///     user: u32::MAX,
    ///     system: 0,
    ///     idle: 0,
    ///     nice: 0,
    /// };
    /// let after = ProcessorTicks {
    ///     user: 1,
    ///     system: 0,
    ///     idle: 0,
    ///     nice: 0,
    /// };
    ///
    /// assert_eq!(LinuxCpu::between(&before, &after).user, 2);
    /// ```
    #[must_use]
    pub fn between(before: &ProcessorTicks, after: &ProcessorTicks) -> Self {
        let ticks = |before: u32, after: u32| u64::from(after.wrapping_sub(before));

        Self {
            user: ticks(before.user, after.user),
            nice: ticks(before.nice, after.nice),
            system: ticks(before.system, after.system),
            idle: ticks(before.idle, after.idle),
            ..Self::default()
        }
    }

    /// The ticks in every state. `top` divides each state's ticks by this
    /// total.
    ///
    /// ```
    /// use procps_core::LinuxCpu;
    ///
    /// let cpu = LinuxCpu {
    ///     user: 1,
    ///     idle: 3,
    ///     ..LinuxCpu::default()
    /// };
    ///
    /// assert_eq!(cpu.total(), 4);
    /// ```
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }
}

impl Add for LinuxCpu {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            user: self.user + other.user,
            nice: self.nice + other.nice,
            system: self.system + other.system,
            idle: self.idle + other.idle,
            iowait: self.iowait + other.iowait,
            irq: self.irq + other.irq,
            softirq: self.softirq + other.softirq,
            steal: self.steal + other.steal,
        }
    }
}

impl Sum for LinuxCpu {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), Add::add)
    }
}
