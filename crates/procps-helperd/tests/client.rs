// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Takes a snapshot through a running helper, as the tools do.

use std::{
    os::unix::net::UnixListener,
    thread,
    time::{Duration, SystemTime},
};

use darwin_proc::Pid;
use pretty_assertions::assert_eq;
use procps_core::{
    Field, FieldGroup, FixtureSource, Process, ProcessSource, Snapshot, SnapshotRequest,
    helper::HelperSource,
};
use procps_helperd::{ConnectionLimits, Listener, Server, TimeLimits};

#[test]
fn a_tool_reads_through_the_helper() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let socket = UnixListener::bind(&path)?;
    let info = Pid::current().info()?;
    let me = info.credentials.euid;
    let mut theirs = info.clone();
    theirs.pid = Pid::from(2);
    let stranger = darwin_proc::Uid::from(me.as_raw() + 1);
    theirs.credentials.ruid = stranger;
    theirs.credentials.euid = stranger;
    theirs.credentials.svuid = stranger;
    let environment = Some(Field::Available(Vec::new()));
    let processes = vec![
        Process {
            environment: environment.clone(),
            ..Process::from_identity(info, Field::Unsupported, Field::Unsupported)
        },
        Process {
            environment,
            ..Process::from_identity(theirs, Field::Unsupported, Field::Unsupported)
        },
    ];
    let taken = SystemTime::UNIX_EPOCH;
    let uptime = Duration::from_secs(1);
    let source = FixtureSource::new(taken, uptime, processes.clone());
    let listener = Listener::new(
        Server::new(source, TimeLimits::default()),
        Duration::from_secs(1),
        ConnectionLimits::default(),
    );
    let request = SnapshotRequest::default().with(FieldGroup::Environment);
    let client = HelperSource::new(&path).owned_by(me);

    let snapshot = thread::scope(|scope| -> Result<_, Box<dyn std::error::Error>> {
        let running = scope.spawn(|| listener.run(&socket));
        let snapshot = client.snapshot(&request)?;
        running
            .join()
            .map_err(|_| "the listener thread panicked")??;

        Ok(snapshot)
    })?;
    let [mine, theirs] =
        <[Process; 2]>::try_from(processes).map_err(|_| "expected two fixture processes")?;

    assert_eq!(
        snapshot,
        Snapshot {
            taken,
            uptime,
            processes: vec![
                Process {
                    environment: Some(Field::Denied),
                    ..theirs
                },
                mine,
            ],
            system: None,
        }
    );

    Ok(())
}
