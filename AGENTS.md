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

- procps-ng is GPL-2.0-or-later, so its code can be translated into this
  project. Check each file's licence header first: GPL-2.0-only code cannot be
  ported.
- A file with code translated from procps-ng has an `SPDX-FileCopyrightText`
  line for each copyright holder in the upstream file, in addition to ours, and
  a comment with the upstream path and version, for example
  `procps-ng v4.0.7 src/pgrep.c`. Its licence is `GPL-3.0-or-later`.
- uutils/procps uses the MIT licence, and its code can be reused. A file with
  code from it keeps the uutils copyright line and uses
  `GPL-3.0-or-later AND MIT`. The first such file also needs
  `reuse download MIT`, which adds `LICENSES/MIT.txt`.

## Architecture

- `darwin-proc` contains every FFI declaration and every `unsafe` block. Each
  `unsafe` block has a `// SAFETY:` comment.
- The tools read process data only through the `ProcessSource` trait, so tests
  can substitute `FixtureSource`.
- A `SnapshotRequest` lists only the field groups that a command needs, because
  reading arguments and environment, and walking memory regions, are slow or
  restricted.
- `procps-helperd` only reads process data. `pkill` sends signals itself with
  `kill(2)`, as the invoking user, so the kernel's permission checks apply.

## Rust style

- `lib.rs`, `mod.rs` and other re-export files contain only module declarations
  and re-exports. `main.rs` sets up observability, parses arguments and
  dispatches. The program logic is in modules.
- Clippy is strict, configured in `[workspace.lints]`. Fix what a lint reports.
  Where a lint does not apply, use `#[expect(lint, reason = "...")]`, which
  warns once the code stops triggering the lint.
- Use private or `pub(crate)` visibility by default, and keep the public APIs of
  the library crates small. Every public item has a doc comment, and every
  public function and method has a doctest.
- Use `cargo add` to add or change dependencies.

## Errors

- Use `thiserror` in every crate, including the binaries, with `#[from]` for
  conversions.
- Each binary has a top-level error enum, and its variants decide the exit
  status. The statuses follow procps-ng: for `pgrep` and `pkill`, a syntax error
  exits with 2 and a fatal error with 3.
- `darwin-proc` errors distinguish a process that has exited, a denied read and
  an unsupported call, because the tools show each case differently.

## Command-line parsing

- `pgrep`, `pkill` and `procps-helperd` use `clap` with derive and parse
  straight into typed values. procps-ng parses `pgrep` and `pkill` with
  `getopt_long`, and `clap` accepts the same forms: clustered short options,
  attached values, abbreviated long options (`infer_long_args`) and options
  after the pattern.
- Before `clap` runs, `pkill` removes the first argument of the form `-<signal>`
  and uses it as the signal, wherever that argument appears. This matches
  procps-ng's `signal_option`. uutils/procps rewrites every such argument, so
  change that part when reusing its code.
- Render `clap` errors and help output in procps-ng's format: glibc's `getopt`
  messages, then procps-ng's usage text, with exit status 2.
- `ps` and `top` use parsers ported from procps-ng, because `ps` mixes UNIX, BSD
  and GNU syntax (`ps aux`, `ps -ef`, `ps opid,cmd`). The ported parsers produce
  typed option structs. From uutils/procps `ps`, reuse the format-specifier
  tables in `mapping.rs`; its `clap` parser rejects `ps aux`.

## Output

- Write through a locked, buffered handle to standard output.
- The tools restore the default `SIGPIPE` disposition at start-up, so a closed
  pipe (`ps aux | head`) terminates them silently, as it terminates procps-ng.
  With Rust's default of ignoring `SIGPIPE`, `println!` panics instead.
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
- `procps-helperd` logs JSON to standard error, which launchd captures.
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
  messages, exit codes and output layout. Add a harness scenario for each
  behaviour that procps-ng defines.

## Development environment

- The flake's development shell provides the Rust toolchain, nightly `rustfmt`,
  `just` and `reuse`, and installs the git hooks. Run commands through it with
  `nix develop -c <command>`, or use direnv with the `.envrc`.
- `nix develop -c just fmt` formats every file with treefmt.
  `nix develop -c just check` runs `nix flake check`, which includes the treefmt
  and `reuse lint` checks.
- Commit messages use Conventional Commits, with bodies wrapped at 72 columns.
  The `commit-msg` hook checks them with wrapscallion.
