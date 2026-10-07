// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Walks the memory regions of live processes.

use std::process::Command;

use assert_matches::assert_matches;
use darwin_proc::{Error, Pid, Protection, Region, Status};
use pretty_assertions::assert_eq;

fn containing(regions: &[Region], address: u64) -> Option<&Region> {
    regions
        .iter()
        .find(|region| (region.address..region.address + region.size).contains(&address))
}

#[test]
fn this_process_has_executable_code_and_a_writable_stack() -> Result<(), Box<dyn std::error::Error>>
{
    let regions = Pid::current().regions()?;
    let code = (containing as fn(&[Region], u64) -> Option<&Region>) as usize as u64;
    let on_the_stack = 0_u8;
    let stack = std::ptr::from_ref(&on_the_stack) as usize as u64;

    let code = containing(&regions, code).ok_or("no region contains the code")?;
    let stack = containing(&regions, stack).ok_or("no region contains the stack")?;

    let read_execute = Protection::from(0x5);
    let read_write = Protection::from(0x3);

    assert_eq!(
        (code.protection, stack.protection, stack.resident > 0),
        (read_execute, read_write, true)
    );

    Ok(())
}

#[test]
fn the_regions_are_ordered_and_do_not_overlap() -> Result<(), Box<dyn std::error::Error>> {
    let regions = Pid::current().regions()?;
    let overlapping: Vec<&[Region]> = regions
        .windows(2)
        .filter(|pair| pair[0].address + pair[0].size > pair[1].address)
        .collect();

    assert_eq!((regions.is_empty(), overlapping), (false, Vec::new()));

    Ok(())
}

#[test]
fn another_users_regions_are_denied() {
    assert_matches!(
        Pid::from(1).regions(),
        Err(Error::Denied { pid, .. }) if pid == Pid::from(1)
    );
}

#[test]
fn a_zombie_has_no_regions() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);

    // Wait without reaping, so that the child stays a zombie.
    while pid.info()?.status != Status::Zombie {
        std::thread::yield_now();
    }

    let regions = pid.regions();
    child.wait()?;

    assert_matches!(regions, Err(Error::Unsupported { pid: zombie, .. }) if zombie == pid);

    Ok(())
}

#[test]
fn an_exited_process_has_no_regions() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);
    child.wait()?;

    assert_matches!(pid.regions(), Err(Error::Exited { pid: exited }) if exited == pid);

    Ok(())
}

#[test]
fn a_walk_can_stop_early() -> Result<(), Box<dyn std::error::Error>> {
    let pid = Pid::current();
    let walked = pid
        .region_walk()?
        .take(2)
        .map(|region| region.map(|region| (region.address, region.size)))
        .collect::<Result<Vec<_>, _>>()?;
    let read: Vec<_> = pid
        .regions()?
        .into_iter()
        .take(2)
        .map(|region| (region.address, region.size))
        .collect();

    assert_eq!(walked, read);

    Ok(())
}

#[test]
fn a_walk_of_an_exited_process_ends_with_the_error() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);
    child.wait()?;

    let walked: Vec<_> = pid.region_walk()?.collect();

    assert_matches!(walked.as_slice(), [Err(Error::Exited { pid: exited })] if *exited == pid);

    Ok(())
}

#[test]
fn a_walk_reads_each_region_when_asked_for_it() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/bin/sleep").arg("60").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);

    let walk = pid.region_walk()?;
    child.kill()?;
    child.wait()?;
    let walked: Vec<_> = walk.collect();

    assert_matches!(walked.as_slice(), [Err(Error::Exited { pid: exited })] if *exited == pid);

    Ok(())
}
