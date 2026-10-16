// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use tempfile::TempDir;

#[cfg(target_os = "macos")]
use crate::fixture::run_until_ready;
use crate::{Error, spawn};

/// Copies of the host binary, one for each process name. A runner and its
/// clones share the copies, which are in one directory. The directory is
/// deleted when the last clone is dropped.
///
/// macOS assesses an executable file before the first process from it starts,
/// and the process waits for the result. The unified log suggests that deleting
/// a file during its assessment can crash syspolicyd. On macOS, `prepare`
/// therefore starts each copy as a fixture and waits until it reports that it
/// is ready before any run uses the copy. Once a preparation has succeeded, the
/// assessment of the copy has ended, so a run that ends early cannot leave it
/// in progress, and later runs start the copy without a new assessment. When a
/// preparation times out, the assessment can still be in progress when the
/// directory is deleted.
///
/// Each process name is prepared once. A run that needs a name whose
/// preparation is in progress waits for it, and a failed preparation fails
/// every run that needs the name with the same error.
///
/// The copies are not hard links to the host. When a hard link to an
/// executable file is deleted, macOS can kill a process that is starting from
/// the same file under another name.
#[derive(Debug)]
pub(crate) struct Programs {
    host: PathBuf,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    directory: Option<TempDir>,
    preparations: HashMap<String, Arc<Preparation>>,
}

/// The result of a process name's preparation, which is set when the
/// preparation ends.
type Preparation = OnceLock<Result<(), Arc<Error>>>;

impl Programs {
    /// Programs that are copies of `host`.
    pub(crate) fn new(host: PathBuf) -> Self {
        Self {
            host,
            state: Mutex::default(),
        }
    }

    /// The directory that contains a program for each of `names`, once each
    /// program's preparation has succeeded.
    pub(crate) fn prepare<'a>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<PathBuf, Error> {
        let (directory, preparations) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let directory = match &mut state.directory {
                Some(directory) => directory.path().to_owned(),
                empty @ None => empty.insert(new_directory()?).path().to_owned(),
            };
            let preparations: Vec<_> = names
                .into_iter()
                .map(|name| {
                    let preparation = state.preparations.entry(name.to_owned()).or_default();
                    (name, Arc::clone(preparation))
                })
                .collect();

            (directory, preparations)
        };

        for (name, preparation) in preparations {
            let program = directory.join(name);
            let prepared =
                preparation.get_or_init(|| self.prepare_one(&program, name).map_err(Arc::new));

            if let Err(source) = prepared {
                return Err(Error::ProgramNotReady {
                    program,
                    source: Arc::clone(source),
                });
            }
        }

        Ok(directory)
    }

    /// Copies the host to `program`, and on macOS runs the copy until it
    /// reports that it is ready.
    #[cfg_attr(
        not(target_os = "macos"),
        expect(unused_variables, reason = "only macOS runs the copy as a fixture")
    )]
    fn prepare_one(&self, program: &Path, name: &str) -> Result<(), Error> {
        // A child that another thread forks during the copy inherits the
        // copy's descriptor until it executes its program, and Linux refuses
        // to execute a file that a process has open for writing.
        let spawning = spawn::lock();
        std::fs::copy(&self.host, program).map_err(|source| Error::Io {
            path: program.to_owned(),
            source,
        })?;
        drop(spawning);

        #[cfg(target_os = "macos")]
        run_until_ready(program, name)?;

        Ok(())
    }
}

/// A new temporary directory that every user can search, so that a run's
/// processes can reach its contents when they run as another user.
pub(crate) fn new_directory() -> Result<TempDir, Error> {
    let directory = tempfile::Builder::new()
        .prefix("procps-harness-")
        .tempdir()
        .map_err(|source| Error::Io {
            path: std::env::temp_dir(),
            source,
        })?;
    let path: &Path = directory.path();

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|source| {
        Error::Io {
            path: path.to_owned(),
            source,
        }
    })?;

    Ok(directory)
}
