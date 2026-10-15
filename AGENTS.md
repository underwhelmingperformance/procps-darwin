<!--
SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>

SPDX-License-Identifier: GPL-3.0-or-later
-->

# Agent rules

## Project

This project reimplements `ps`, `top`, `pgrep` and `pkill` from procps-ng 4.0.7
for macOS 27 on arm64, in Rust. `PLAN.md` contains the audit of the macOS tools,
the data that macOS exposes, the planned crates and the task list. Read it
before starting a task, and update it when a decision in it changes.

Use British English for project-owned prose.

Where these rules differ from general Rust guidance, these rules apply. The
differences are deliberate: `thiserror` in place of `anyhow`, parsers ported
from procps-ng for `ps` and `top` in place of `clap`, and no OpenTelemetry
exporter.

## Compatibility

- procps-ng 4.0.7 is the specification. Where macOS provides the data, match its
  output, error messages and exit codes byte for byte, because scripts written
  for Linux parse them.
- When procps-ng's behaviour is unclear, read its source or run it in the
  reference harness. The man pages leave out details such as column widths and
  error messages.
- Data that the caller is not permitted to read, and columns for Linux concepts
  that macOS lacks, show `-`. Selection and action options for Linux concepts
  exit with status 2, so that `pkill --cgroup` never signals processes that it
  could not select properly. `PLAN.md` lists the full compatibility rules.
- Record every approximation of a Linux concept in `docs/mappings.md`, with the
  Darwin source of the value.

## Licensing and provenance

- Every file starts with `SPDX-FileCopyrightText` and `SPDX-License-Identifier`
  headers, following the REUSE specification. Add them with:

  ```sh
  nix develop -c reuse annotate --copyright "<holder>" --year <year> \
    --license GPL-3.0-or-later <file>
  ```

  For a file that cannot contain comments, such as JSON, add an entry to
  `REUSE.toml`. `nix flake check` runs `reuse lint`.

- Files that a tool generates, such as `Cargo.lock` and `flake.lock`, have no
  copyright holder. List them in the generated-files entry of `REUSE.toml`,
  which sets `SPDX-FileCopyrightText = "NONE"` and CC0-1.0, and never annotate
  them with our copyright.

- A golden file with text from procps-ng's source has the copyright holders and
  the licence of the procps-ng file that the text comes from. The usage text of
  `pgrep` and `pkill` comes from `src/pgrep.c`, which is GPL-2.0-or-later. The
  usage text of `ps` comes from `src/ps/help.c`, and the listing of `ps L` from
  the table of format specifiers in `src/ps/output.c`, which are both
  LGPL-2.1-or-later. List each golden file in the matching entry of
  `REUSE.toml`. The `reuse` test in `procps-harness` fails when those entries
  differ from the golden files with that text.
- procps-ng is GPL-2.0-or-later, and its library is LGPL-2.1-or-later, so its
  code can be translated into this project. Check each file's licence header
  first: GPL-2.0-only code cannot be ported.
- A file with code translated from procps-ng has an `SPDX-FileCopyrightText`
  line for each copyright holder in the upstream file, in addition to ours, and
  a comment with the upstream path and version, for example
  `procps-ng v4.0.7 src/pgrep.c`. Its licence is `GPL-3.0-or-later`.
- Code copied from another project, such as glibc's regular expression code in
  `crates/darwin-proc/glibc-regex`, keeps its own headers, and each change to it
  is marked `procps-darwin`. An entry in `REUSE.toml` records its copyright and
  licence.
- uutils/procps uses the MIT licence, and its code can be reused. A file with
  code from it keeps the uutils copyright line and uses
  `GPL-3.0-or-later AND MIT`. The first such file also needs
  `reuse download MIT`, which adds `LICENSES/MIT.txt`.

## Architecture

- `darwin-proc` contains every FFI declaration and every `unsafe` block. Each
  `unsafe` block has a `// SAFETY:` comment. The workspace denies `unsafe_code`,
  and `darwin-proc` opts out with `#![expect(unsafe_code, reason = "...")]` at
  its root.
- The tools read process data only through the `ProcessSource` trait, so tests
  can substitute `FixtureSource`. The exception is `pkill`'s read of each
  process's start time just before it sends a signal, which always asks the
  local kernel, because `kill(2)` acts on the kernel's processes at that moment.
  Tests replace that read through the `Signaller` trait.
- A `SnapshotRequest` lists only the field groups that a command needs, because
  reading arguments and environment, and walking memory regions, are slow or
  restricted.
- `procps-helperd` only reads process data. `pkill` sends signals itself with
  `kill(2)`, as the invoking user, so the kernel's permission checks apply.

## Rust style

- `lib.rs`, `mod.rs` and other re-export files contain only module declarations
  and re-exports. A binary's `main` sets up observability, parses arguments and
  dispatches. When several binaries share that work, each `main` calls one
  library function: `pgrep` and `pkill` both call `procps::pgrep::entry`. The
  program logic is in modules.
- Clippy is strict, configured in `[workspace.lints]`. Fix what a lint reports.
  Where a lint does not apply, use `#[expect(lint, reason = "...")]`, which
  warns once the code stops triggering the lint. Clippy rejects `#[allow]`.
- `missing_docs` applies to every crate, so each integration test, example and
  benchmark file starts with a `//!` doc comment.
- Use private or `pub(crate)` visibility by default, and keep the public APIs of
  the library crates small. Every public item has a doc comment, and every
  public function and method has a doctest.
- Every dependency is declared once, in `[workspace.dependencies]` in the root
  `Cargo.toml`, and each crate refers to it with `workspace = true`. `cargo add`
  cannot edit the workspace table, so add or change an entry there by hand, then
  run `cargo add -p <crate> <dependency>`, which uses the workspace entry.

## Errors

- Use `thiserror` in every crate, including the binaries, with `#[from]` for
  conversions.
- Each binary, or each set of binaries that share an entry function, has a
  top-level error enum, and its variants decide the exit status. The statuses
  follow procps-ng: for `pgrep` and `pkill`, a syntax error exits with 2 and a
  fatal error with 3.
- `darwin-proc` errors distinguish a process that has exited, a denied read and
  an unsupported call, because the tools show each case differently.

## Command-line parsing

- `pgrep`, `pkill` and `procps-helperd` use `clap`. `procps-helperd` uses
  `clap`'s derive and parses straight into typed values. procps-ng parses
  `pgrep` and `pkill` with `getopt_long`, and `clap` accepts the same forms:
  clustered short options, attached values, abbreviated long options
  (`infer_long_args`) and options after the pattern. `pgrep` and `pkill` build
  their `clap` command from one table of options, in the order of procps-ng's
  `longopts`. The same table gives each tool its own short options and drives a
  scanner that reproduces glibc's messages. After `clap` has parsed the command
  line, the parser replays the options in command-line order. The replay needs
  the positions of the occurrences, which `clap`'s derive API does not give.
- Before `clap` runs, `pkill` removes the first argument of the form `-<signal>`
  and uses it as the signal, wherever that argument appears. This matches
  procps-ng's `signal_option`. uutils/procps rewrites every such argument, so
  change that part when reusing its code.
- Render `clap` errors in procps-ng's format: glibc's `getopt` message, then
  procps-ng's usage text, with exit status 2. The help output is procps-ng's
  usage text, with exit status 0.
- `ps` and `top` use parsers ported from procps-ng, because `ps` mixes UNIX, BSD
  and GNU syntax (`ps aux`, `ps -ef`, `ps opid,cmd`). The ported parsers produce
  typed option structs. uutils/procps `ps` is not reused: its `clap` parser
  rejects `ps aux`, and the table of format specifiers in its `mapping.rs` has
  only their headers. `ps` uses a translation of the table in procps-ng's
  `src/ps/output.c`, which also has each specifier's width and flags, the macros
  and the AIX descriptors.

## Output

- Write through `procps::output::Output`. It buffers standard output as C does
  and calls `write(2)` itself, because Rust's standard streams treat `EBADF` as
  a success.
- Before `main` runs, Rust's runtime ignores `SIGPIPE` and opens `/dev/null` in
  place of a standard stream that is closed. The tools call
  `darwin_proc::restore_inherited_state` at start-up. It restores the `SIGPIPE`
  disposition that the tool inherited, so a closed pipe (`pgrep -a . | head`)
  terminates `pgrep` and `pkill` silently, as it terminates procps-ng's, unless
  the parent ignores `SIGPIPE`. procps-ng's `ps` installs handlers for most
  signals, so `ps` installs the same handlers with `darwin_proc::catch_signals`,
  and a closed pipe makes it exit with status 0. `restore_inherited_state` also
  replaces a standard output or standard error that was closed with `/dev/null`
  opened for reading only, so that writes to it fail with `EBADF`. procps-ng's
  `pgrep` and `pkill` open an epoll descriptor, which takes the lowest closed
  descriptor. Our `pgrep` and `pkill` therefore use
  `Output::with_epoll_descriptor`, which fails writes to that stream with
  `EINVAL`. procps-ng's `ps` has no epoll descriptor, so `ps` uses
  `Output::new`, whose writes to a closed stream fail with `EBADF`.
- The helper client sets `SO_NOSIGPIPE` on its socket, so a helper that closes
  the connection causes an error in the client, not a `SIGPIPE`.

## Observability

- Add a `tracing` span to each `darwin-proc` call that reads from the kernel,
  each helper request, and each `top` refresh. Use events for errors that the
  tools recover from, such as a process that exits during a read.
- The tools install a subscriber only when `PROCPS_DARWIN_LOG` is set, using it
  as an `EnvFilter` directive. The subscriber writes to standard error, in a
  pretty format on a terminal and as JSON otherwise. With the variable unset,
  the tools print only what procps-ng prints.
- `procps-helperd` logs JSON to standard error, which launchd writes to
  `/var/log/procps-helperd.log`.
- Use `tracing-subscriber` only. Nothing collects OpenTelemetry traces, so leave
  out `tracing-opentelemetry`.

## Testing

- Use red/green TDD: write a failing test that describes the behaviour, then
  make it pass.
- Unit tests run against `FixtureSource`, because the live process table changes
  between runs.
- Use `pretty_assertions::assert_eq!` for equality, `rstest` for parameterised
  cases, and `assert_matches!` for error variants and other enums. In tests, use
  `?` or `expect()` with a useful message in place of `unwrap()`.
- Assert on whole values (records, slices, output strings), so a field that
  changes or goes missing shows up in the failure.
- The procps-ng reference harness is the oracle for option parsing, error
  messages, exit codes and output layout. Add a scenario for each behaviour that
  procps-ng defines, in `crates/procps-harness/scenarios/<tool>/`, then run
  `nix develop -c just harness-generate`, which needs Docker, and commit the
  golden file that it writes. Mark a scenario `pending = true` until the tool
  matches procps-ng; the `reference` test in the `procps` package fails when a
  pending scenario starts to match, so remove the mark then.
- Each scenario runs in its own session, and its command must select only
  processes in that session, with `pgrep -s 0`, `ps -p {a1}` or a similar
  option. Scenarios then run in parallel without seeing each other's fixtures.
  Generation rejects a scenario whose outcome changes when processes with the
  fixtures' names run outside the session.
- A scenario's command must not stop the session leader. The harness reports a
  stop that it sees, but it can miss a stop that a `SIGCONT` undoes at once, and
  the outcome then depends on timing.
- A scenario's command runs with `LC_ALL=C` and `TZ=UTC`. An `[environment]`
  table in the scenario sets other variables, such as `POSIXLY_CORRECT`, and can
  replace `TZ`. When the table sets `LANG` or an `LC_` variable, the harness
  leaves out `LC_ALL=C`. The table must not set `PATH` or `HOME`, because the
  harness needs them to find the tools and to give each run an empty home
  directory. The reference container has only the `C`, `POSIX` and `C.UTF-8`
  locales and no time zone files, so a locale must be one of those three, or
  `C.utf8`, which glibc and the tools read as `C.UTF-8`. `TZ` must be `UTC` or a
  POSIX `TZ` string such as `JST-9` or `GMT0BST,M3.5.0/1,M10.5.0` that glibc and
  macOS read in the same way: summer time needs two `M` rules in months 2 to 11,
  at least two months apart.

## Development environment

- The flake's development shell provides the Rust toolchain, nightly `rustfmt`,
  `just`, `cargo-deny`, `cargo-audit` and `reuse`, and installs the git hooks.
  Run commands through it with `nix develop -c <command>`, or use direnv with
  the `.envrc`.
- `nix develop -c just test` runs the tests. `just clippy`, `just doc`,
  `just deny`, `just audit` and `just reuse` run one check each, and `just fmt`
  formats every file with treefmt.
- `nix develop -c just check` runs clippy, the tests, the documentation build,
  `cargo deny`, `cargo audit` and `reuse lint`, then `nix flake check`. The
  flake check builds the workspace with Nix and repeats those checks, in the dev
  profile as locally, and runs treefmt. Its `glibc-ctype` check regenerates
  `crates/procps/src/ps/glibc_ctype.rs` from the patched source of the glibc in
  the flake's nixpkgs, which the reference container uses, and fails when the
  committed file differs. `just glibc-ctype` writes the regenerated file. The
  flake check's audit reads the RustSec advisory database pinned in
  `flake.lock`. Renovate's weekly lock-file maintenance is configured to update
  it, and `nix flake update advisory-db` updates it by hand. `just audit` reads
  the live database. CI runs `nix flake check`.
- The `pre-commit` hook formats the changed files and runs clippy on every
  commit. The `pre-push` hook runs `nix flake check` once for each push, on the
  first ref that the push does not delete, which can be a branch or a tag. A ref
  whose previous commit the local repository lacks, such as a new branch, counts
  only if it has commits that no remote-tracking branch of that remote contains.
  CI checks each pull request and `main`. The tests are not part of the
  `pre-commit` hook, so run `just test` before each commit.
- `just clippy` also checks `procps-harness` for `aarch64-unknown-linux-gnu`,
  because the harness generates the golden files on Linux, and some of its code
  compiles only there.
- Commit messages use Conventional Commits, with bodies wrapped at 72 columns.
  The `commit-msg` hook checks them with wrapscallion.
