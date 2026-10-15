// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2023 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 1998-2002 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/global.c.

use std::{ffi::OsStr, io::Write, process::ExitCode};

use darwin_proc::{CalendarTime, Locale, Pid};
use procps_core::{Charset, SourceChoice};

use crate::{
    program::PROCPS_NG_VERSION,
    ps::{
        display::{self, Invocation},
        error::PsError,
        help,
        options::{Command, Info, Options},
        specifier,
        system::System,
    },
};

/// What `--info` reports about `ps` itself, which `ps` reads from the
/// kernel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Itself {
    /// The effective user ID.
    euid: u32,
    /// The device number of the controlling terminal, or `None` without one.
    terminal: Option<libc::dev_t>,
    /// The size of a memory page.
    page_size: usize,
}

impl Itself {
    /// The values for the calling process.
    fn current() -> Self {
        let terminal = Pid::current()
            .info()
            .ok()
            .and_then(|info| info.terminal)
            .map(|terminal| terminal.device);

        Self {
            euid: rustix::process::geteuid().as_raw(),
            terminal,
            page_size: rustix::param::page_size(),
        }
    }
}

/// The text that procps-ng's `self_info` prints for `--info`. procps-ng also
/// prints the versions of glibc and GCC that it was compiled with, which `ps`
/// leaves out. The terminal's major and minor numbers are Darwin's, and are 0
/// without a terminal, as in procps-ng. The effective user ID is printed as a
/// signed number, as procps-ng's `%d` prints it.
fn info_text(info: &Info, itself: Itself) -> Vec<u8> {
    let personality = &info.personality;
    let unix = |format: Option<&str>| format.unwrap_or("(none)").to_owned();
    let device = itself.terminal.map_or(0, i64::from);
    let (major, minor) = ((device >> 24) & 0xff, device & 0x00ff_ffff);

    [
        format!(
            "BSD j    {}\nBSD l    {}\nBSD s    {}\nBSD u    {}\nBSD v    {}\n",
            personality.bsd.j,
            personality.bsd.l,
            personality.bsd.s,
            personality.bsd.u,
            personality.bsd.v
        )
        .into_bytes(),
        format!(
            "SysV -f  {}\nSysV -fl {}\nSysV -j  {}\nSysV -l  {}\n\n",
            unix(personality.unix.f),
            unix(personality.unix.fl),
            unix(personality.unix.j),
            unix(personality.unix.l)
        )
        .into_bytes(),
        format!(
            "procps-darwin version {} (procps-ng {PROCPS_NG_VERSION})\n\n",
            env!("CARGO_PKG_VERSION")
        )
        .into_bytes(),
        format!(
            "header_gap=-1 lines_to_next_header=1\nscreen_cols={} screen_rows={}\n\n",
            info.screen.columns, info.screen.rows
        )
        .into_bytes(),
        format!("personality=0x{:08x} (from \"", personality.flags.bits()).into_bytes(),
        personality.name.clone(),
        format!(
            "\")\nEUID={} TTY={major},{minor} page_size={}\n",
            itself.euid.cast_signed(),
            itself.page_size
        )
        .into_bytes(),
        b"sizeof(proc_t)=8 sizeof(long)=8 sizeof(long)=8\narchdefs:\n".to_vec(),
    ]
    .concat()
}

/// Lists the processes that `options` select, from the source that
/// `PROCPS_DARWIN_SOURCE` and the helper decide, and returns the exit status:
/// 0 if `ps` listed a process, and 1 otherwise, as in procps-ng.
fn list(
    options: &Options,
    system: &dyn System,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<ExitCode, PsError> {
    let invocation = Invocation {
        myself: Pid::current(),
        euid: rustix::process::geteuid().as_raw(),
        charset: Charset::from_codeset(Locale::current().codeset()),
        page_size: u64::try_from(rustix::param::page_size()).unwrap_or(16_384),
        calendar: CalendarTime::local,
    };
    let source = SourceChoice::current().choose()?;

    if display::list(options, &invocation, &source, system, stdout, stderr)? {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

/// Does what `command` asks `ps` to do, and returns the exit status.
/// `argument_zero` is `argv[0]`, and `program` is the name that `ps` was run
/// as, without its directory. `system` gives the names of users and groups.
pub(crate) fn main(
    argument_zero: &OsStr,
    program: &OsStr,
    command: &Command,
    system: &dyn System,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> ExitCode {
    // A failed write is reported when `ps` closes its output, as procps-ng's
    // `close_stdout` reports it.
    let _ = match command {
        Command::Help(section) => stdout.write_all(&help::usage(program, *section)),
        Command::Version => stdout.write_all(&crate::program::version(program)),
        // procps-ng prints each line with its own `printf`, and goes on after
        // a failed one. When standard output is closed, the last lines stay in
        // the buffer, and closing the stream reports the error's description.
        Command::ListSpecifiers => {
            for line in specifier::listing() {
                let _ = stdout.write_all(&line);
            }

            Ok(())
        }
        Command::Info(info) => stderr.write_all(&info_text(info, Itself::current())),
        Command::Run(options) => {
            return list(options, system, stdout, stderr)
                .unwrap_or_else(|error| error.report(argument_zero, program, stderr));
        }
    };

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Itself, info_text};
    use crate::ps::{
        options::{Info, Screen},
        personality::Personality,
        system::testing::FakeSystem,
    };

    #[rstest]
    #[case::user(501, Some(0x1000_0003), "EUID=501 TTY=16,3")]
    #[case::large_user(4_294_967_294, None, "EUID=-2 TTY=0,0")]
    fn info_has_procps_ng_s_layout(
        #[case] euid: u32,
        #[case] terminal: Option<libc::dev_t>,
        #[case] identity: &str,
    ) {
        let info = Info {
            personality: Personality::from_environment(&FakeSystem::default()),
            screen: Screen {
                columns: 131_072,
                rows: 24,
            },
        };
        let itself = Itself {
            euid,
            terminal,
            page_size: 16384,
        };
        let expected = format!(
            "BSD j    OL_j\nBSD l    OL_l\nBSD s    OL_s\nBSD u    OL_u\nBSD v    OL_v\n\
             SysV -f  (none)\nSysV -fl (none)\nSysV -j  (none)\nSysV -l  (none)\n\n\
             procps-darwin version {} (procps-ng 4.0.7)\n\n\
             header_gap=-1 lines_to_next_header=1\nscreen_cols=131072 screen_rows=24\n\n\
             personality=0x00000000 (from \"unknown\")\n{identity} page_size=16384\n\
             sizeof(proc_t)=8 sizeof(long)=8 sizeof(long)=8\narchdefs:\n",
            env!("CARGO_PKG_VERSION")
        );

        assert_eq!(String::from_utf8(info_text(&info, itself)), Ok(expected));
    }
}
