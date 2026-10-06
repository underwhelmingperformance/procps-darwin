// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! A root helper daemon that reads process data on behalf of unprivileged
//! users of the procps tools.

use std::{path::PathBuf, process::ExitCode, time::Duration};

use clap::Parser;
use procps_helperd::{Config, ErrorChain, TimeLimits};
use tracing_subscriber::EnvFilter;

/// The variable that sets which events the helper logs.
const LOG: &str = "PROCPS_DARWIN_LOG";

/// Serves process data to the procps tools.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Listen on a socket at this path, instead of the socket that launchd
    /// passes.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Stop this many seconds after starting or after the last connection
    /// closes, whichever is later.
    #[arg(long, default_value = "60", value_parser = seconds)]
    idle_timeout: Duration,
    /// Give the helper this many seconds, including any wait for a worker, to
    /// read the process data for a request, and a client this many seconds to
    /// receive the response.
    #[arg(long, default_value = "10", value_parser = seconds)]
    time_limit: Duration,
}

/// Parses a whole number of seconds from 1 to 3600.
fn seconds(value: &str) -> Result<Duration, String> {
    match value.parse::<u64>() {
        Ok(seconds @ 1..=3600) => Ok(Duration::from_secs(seconds)),
        _ => Err(format!(
            "{value} is not a whole number of seconds from 1 to 3600"
        )),
    }
}

fn main() -> ExitCode {
    let filter = EnvFilter::try_from_env(LOG);

    tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .with_env_filter(
            filter
                .as_ref()
                .map_or_else(|_| EnvFilter::new("info"), Clone::clone),
        )
        .init();

    if let (Err(error), Some(_)) = (&filter, std::env::var_os(LOG)) {
        tracing::warn!(error = %ErrorChain(error), "ignoring {LOG}");
    }

    let arguments = Arguments::parse();
    let config = Config {
        socket: arguments.socket,
        idle: arguments.idle_timeout,
        time_limits: TimeLimits {
            snapshot: arguments.time_limit,
            response: arguments.time_limit,
            ..TimeLimits::default()
        },
        ..Config::default()
    };

    match config.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %ErrorChain(&error), "stopping");
            ExitCode::FAILURE
        }
    }
}
