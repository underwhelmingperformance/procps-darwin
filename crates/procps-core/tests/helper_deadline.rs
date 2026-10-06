// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Limits the time that a connection's reads and writes may take together.

use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    time::Duration,
};

use pretty_assertions::assert_eq;
use procps_core::helper::Deadline;

#[test]
fn a_closed_connection_ends_reads_and_writes() -> io::Result<()> {
    let (client, theirs) = UnixStream::pair()?;
    drop(client);
    let mut connection = Deadline::new(&theirs, Duration::from_secs(1));

    let read = connection.read(&mut [0; 1]).map_err(|error| error.kind());
    let written = connection.write(&[0]).map_err(|error| error.kind());

    assert_eq!((read, written), (Ok(0), Err(io::ErrorKind::BrokenPipe)));

    Ok(())
}

#[test]
fn a_connection_that_has_used_its_time_times_out() -> io::Result<()> {
    let (_client, theirs) = UnixStream::pair()?;
    let mut connection = Deadline::new(&theirs, Duration::ZERO);

    let read = connection.read(&mut [0; 1]).map_err(|error| error.kind());

    assert_eq!(read, Err(io::ErrorKind::TimedOut));

    Ok(())
}

#[test]
fn a_closed_connection_still_returns_the_data_sent_before() -> io::Result<()> {
    let (mut client, theirs) = UnixStream::pair()?;
    client.write_all(b"ok")?;
    drop(client);
    let mut received = Vec::new();

    Deadline::new(&theirs, Duration::from_secs(1)).read_to_end(&mut received)?;

    assert_eq!(received, b"ok");

    Ok(())
}
