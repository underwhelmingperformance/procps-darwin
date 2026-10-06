// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    io,
    process::{Child, Command},
    sync::{Mutex, MutexGuard, PoisonError},
};

/// Held while this process starts a child process.
///
/// On macOS, std creates a pipe and then marks its ends close-on-exec in a
/// separate call. A child that another thread starts between the two calls
/// inherits the pipe. If that child outlives the run that created the pipe,
/// the reader at the other end never sees end-of-file. Fixtures and session
/// leaders are long-lived, and the tests start runs on several threads.
static SPAWNING: Mutex<()> = Mutex::new(());

/// Waits until no other thread of this process is starting a child.
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    SPAWNING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts a command while no other thread of this process starts one.
pub(crate) trait SpawnAlone {
    /// Like [`Command::spawn`], but holding [`lock`].
    fn spawn_alone(&mut self) -> io::Result<Child>;
}

impl SpawnAlone for Command {
    fn spawn_alone(&mut self) -> io::Result<Child> {
        let _spawning = lock();

        self.spawn()
    }
}
