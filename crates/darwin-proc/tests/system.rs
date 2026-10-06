// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads system-wide statistics.

use std::{
    process::Command,
    time::{Duration, Instant, SystemTime},
};

use darwin_proc::{Host, ProcessInfo};
use pretty_assertions::assert_eq;

/// Runs `sysctl -n` for `name` and returns its output without the trailing
/// newline.
fn sysctl(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("/usr/sbin/sysctl")
        .args(["-n", name])
        .output()?;

    Ok(String::from_utf8(output.stdout)?.trim_end().to_owned())
}

#[test]
fn the_boot_time_matches_sysctl() -> Result<(), Box<dyn std::error::Error>> {
    // The output starts with `{ sec = 1789727921, usec = 68321 }`.
    let printed = sysctl("kern.boottime")?;
    let fields: Vec<u64> = printed
        .split(|character: char| !character.is_ascii_digit())
        .filter(|field| !field.is_empty())
        .take(2)
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    let [seconds, microseconds] = fields[..] else {
        return Err(format!("unexpected kern.boottime: {printed}").into());
    };

    assert_eq!(
        Host::boot_time()?,
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds) + Duration::from_micros(microseconds)
    );

    Ok(())
}

#[test]
fn the_load_averages_are_non_negative() -> Result<(), Box<dyn std::error::Error>> {
    let load = Host::load_average()?;
    let values = [load.one_minute, load.five_minutes, load.fifteen_minutes];

    assert_eq!(
        values.map(|value| value.is_finite() && value >= 0.0),
        [true; 3],
        "{load:?}"
    );

    Ok(())
}

#[test]
fn the_memory_total_matches_sysctl_and_bounds_each_category()
-> Result<(), Box<dyn std::error::Error>> {
    let memory = Host::memory()?;

    // The kernel does not read the page counts atomically, so their sum can
    // exceed the total. The test therefore checks each count against the
    // total separately.
    let categories = [
        memory.free,
        memory.active,
        memory.inactive,
        memory.speculative,
        memory.wired,
        memory.compressor,
    ];

    assert_eq!(
        (
            memory.total.to_string(),
            categories.map(|category| category <= memory.total)
        ),
        (sysctl("hw.memsize")?, [true; 6]),
        "{memory:?}"
    );

    Ok(())
}

#[test]
fn the_swap_space_adds_up() -> Result<(), Box<dyn std::error::Error>> {
    let swap = Host::swap()?;

    assert_eq!(swap.used + swap.available, swap.total, "{swap:?}");

    Ok(())
}

#[test]
fn every_processor_reports_ticks_that_only_increase() -> Result<(), Box<dyn std::error::Error>> {
    let before = Host::processors()?;
    std::thread::sleep(Duration::from_millis(50));
    let after = Host::processors()?;

    let increasing: Vec<bool> = before
        .iter()
        .zip(&after)
        .map(|(before, after)| {
            after.user >= before.user
                && after.system >= before.system
                && after.idle >= before.idle
                && after.nice >= before.nice
        })
        .collect();

    assert_eq!(
        (before.len().to_string(), after.len(), increasing),
        (sysctl("hw.ncpu")?, before.len(), vec![true; before.len()])
    );

    Ok(())
}

#[test]
fn the_task_count_is_close_to_the_process_count() -> Result<(), Box<dyn std::error::Error>> {
    let totals = Host::task_totals()?;
    let processes = ProcessInfo::all()?.len();

    // Processes start and exit between the two reads, and a zombie has a
    // process entry but no task.
    assert_eq!(
        (
            totals.tasks.abs_diff(processes) < 50,
            totals.threads >= totals.tasks
        ),
        (true, true),
        "{totals:?} against {processes} processes"
    );

    Ok(())
}

#[test]
fn the_logged_in_users_match_who() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("/usr/bin/who").output()?;
    let sessions = String::from_utf8(output.stdout)?.lines().count();

    assert_eq!(Host::logged_in_users(), sessions);

    Ok(())
}

#[test]
fn the_uptime_advances_with_the_monotonic_clock() -> Result<(), Box<dyn std::error::Error>> {
    let start = (Host::uptime()?, Instant::now());
    std::thread::sleep(Duration::from_millis(100));
    let end = (Host::uptime()?, Instant::now());
    let advanced = end.0.checked_sub(start.0).ok_or("the uptime decreased")?;
    let elapsed = end.1 - start.1;

    assert!(
        advanced.abs_diff(elapsed) < Duration::from_millis(50),
        "the uptime advanced by {advanced:?} in {elapsed:?}"
    );

    Ok(())
}
