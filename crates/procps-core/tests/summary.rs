// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Maps Darwin's system statistics onto `top`'s Linux summary figures.

use darwin_proc::{Memory, ProcessorTicks, Swap};
use pretty_assertions::assert_eq;
use procps_core::{LinuxCpu, LinuxMemory, LinuxSwap};
use rstest::rstest;

/// Memory with `total` bytes and the given amounts of free, speculative,
/// file-backed and purgeable memory, in bytes. The speculative memory is part
/// of the file-backed memory, as in XNU's counts.
fn memory(total: u64, free: u64, speculative: u64, file_backed: u64, purgeable: u64) -> Memory {
    Memory {
        total,
        free,
        active: 0,
        inactive: 0,
        speculative,
        throttled: 0,
        wired: 0,
        purgeable,
        file_backed,
        anonymous: 0,
        compressor: 0,
        compressed: 0,
    }
}

#[rstest]
#[case::typical(
    memory(1000, 100, 20, 300, 30),
    LinuxMemory { total: 1000, free: 100, used: 570, buff_cache: 330, available: 430 }
)]
#[case::available_above_the_total(
    memory(1000, 100, 0, 700, 300),
    LinuxMemory { total: 1000, free: 100, used: 900, buff_cache: 1000, available: 100 }
)]
fn memory_follows_procps_ng(#[case] memory: Memory, #[case] expected: LinuxMemory) {
    assert_eq!(LinuxMemory::from(&memory), expected);
}

#[rstest]
#[case::in_use(
    Swap { total: 1000, used: 400, available: 600, encrypted: true },
    LinuxSwap { total: 1000, free: 600, used: 400 }
)]
#[case::none(
    Swap { total: 0, used: 0, available: 0, encrypted: false },
    LinuxSwap { total: 0, free: 0, used: 0 }
)]
fn swap_follows_procps_ng(#[case] swap: Swap, #[case] expected: LinuxSwap) {
    assert_eq!(LinuxSwap::from(&swap), expected);
}

/// Ticks for one processor.
const fn ticks(user: u32, system: u32, idle: u32) -> ProcessorTicks {
    ProcessorTicks {
        user,
        system,
        idle,
        nice: 0,
    }
}

#[rstest]
#[case::counting(ticks(10, 20, 30), ticks(15, 22, 40), (5, 2, 10))]
#[case::wrapping(ticks(u32::MAX - 1, 0, 0), ticks(3, 0, 0), (5, 0, 0))]
fn cpu_time_is_the_difference_between_two_readings(
    #[case] before: ProcessorTicks,
    #[case] after: ProcessorTicks,
    #[case] (user, system, idle): (u64, u64, u64),
) {
    assert_eq!(
        LinuxCpu::between(&before, &after),
        LinuxCpu {
            user,
            system,
            idle,
            ..LinuxCpu::default()
        }
    );
}

#[test]
fn the_cpu_time_of_every_processor_adds_up() {
    let before = [ticks(0, 0, 0), ticks(10, 10, 10)];
    let after = [ticks(1, 2, 3), ticks(14, 15, 16)];
    let total: LinuxCpu = before
        .iter()
        .zip(&after)
        .map(|(before, after)| LinuxCpu::between(before, after))
        .sum();

    assert_eq!(
        (total, total.total()),
        (
            LinuxCpu {
                user: 5,
                system: 7,
                idle: 9,
                ..LinuxCpu::default()
            },
            21
        )
    );
}
