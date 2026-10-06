// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Identifies the peer of a Unix-domain socket and asks launchd for sockets.

use std::{fs::File, os::unix::net::UnixStream};

use assert_matches::assert_matches;
use darwin_proc::{Call, Error, LaunchdSockets, Peer, Pid};
use pretty_assertions::assert_eq;

#[test]
fn the_peer_of_a_socket_pair_is_this_process() -> Result<(), Box<dyn std::error::Error>> {
    let (ours, _theirs) = UnixStream::pair()?;
    let credentials = Pid::current().info()?.credentials;

    assert_eq!(
        Peer::of(&ours)?,
        Peer {
            uid: credentials.euid,
            gid: credentials.egid,
            pid: Pid::current(),
        }
    );

    Ok(())
}

#[test]
fn a_process_that_launchd_did_not_start_has_no_sockets() {
    assert_matches!(
        LaunchdSockets::activate(c"Listeners"),
        Err(Error::Os {
            call: Call::LaunchdSockets,
            ..
        })
    );
}

#[test]
fn a_closed_socket_has_no_peer_process() -> Result<(), Box<dyn std::error::Error>> {
    let (ours, theirs) = UnixStream::pair()?;
    drop(theirs);

    assert_matches!(
        Peer::of(&ours),
        Err(Error::Os {
            call: Call::PeerProcess,
            ..
        })
    );

    Ok(())
}

#[test]
fn a_file_has_no_peer() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open("/dev/null")?;

    assert_matches!(
        Peer::of(&file),
        Err(Error::Os {
            call: Call::PeerCredentials,
            ..
        })
    );

    Ok(())
}
