# Agent rules

## Project

This project reimplements `ps`, `top`, `pgrep` and `pkill` from procps-ng 4.0.7
for macOS 27 on arm64, in Rust. `PLAN.md` contains the audit of the macOS tools,
the data that macOS exposes, the architecture and the task list. Read it before
starting a task, and update it when a decision in it changes.

The project's licence is GPL-3.0-or-later. Use British English for project-owned
prose.

## Compatibility

- procps-ng 4.0.7 is the specification. Where macOS provides the data, match its
  output, error messages and exit codes byte for byte.
- When procps-ng's behaviour is unclear, read its source or run it in the
  reference harness. The man pages leave out details such as column widths and
  error messages.
- Data that the caller is not permitted to read, and columns for Linux concepts
  that macOS lacks, show `-`. Selection or action options for Linux concepts
  exit with status 2. `PLAN.md` lists the full compatibility rules.
- Record every approximation of a Linux concept in `docs/mappings.md`, with the
  Darwin source of the value.

## Porting code

- procps-ng is GPL-2.0-or-later, so its code can be translated into this
  project. Check each file's licence header first: GPL-2.0-only code cannot be
  ported.
- uutils/procps uses the MIT licence, and its code can be reused.
- A file that contains translated or copied code keeps the original copyright
  notices in its header, together with the path of the upstream file.

## Architecture

- `darwin-proc` contains every FFI declaration and every `unsafe` block. Each
  `unsafe` block has a `// SAFETY:` comment.
- The tools read process data through the `ProcessSource` trait and never call
  `darwin-proc` directly. Tests substitute `FixtureSource`.
- A `SnapshotRequest` lists only the field groups that a command needs.
  Arguments, environment and memory-region walks are slow or restricted.
- `procps-helperd` only reads process data. `pkill` sends signals itself with
  `kill(2)`, as the invoking user, so the kernel's permission checks apply.

## Rust style

- `lib.rs`, `mod.rs` and other re-export files contain only module declarations
  and re-exports. `main.rs` sets up observability, parses arguments and
  dispatches; the program logic is in modules.
- Clippy is strict, configured in `[workspace.lints]`. Fix what a lint reports.
  Where a lint does not apply, use `#[expect(lint, reason = "...")]`, never
  `#[allow]`.
- Use private or `pub(crate)` visibility by default, and keep the public APIs of
  the library crates small. Every public item has a doc comment, and every
  public function and method has a doctest.
- Use `cargo add` to add or change dependencies.

## Errors

- Use `thiserror` in every crate, including the binaries. Use `#[from]` for
  conversions, so `map_err` is not needed.
- Each binary has a top-level error enum. Its variants decide the exit status,
  which follows procps-ng: for `pgrep` and `pkill`, a syntax error exits with 2
  and a fatal error with 3.
- `darwin-proc` errors distinguish a process that has exited, a denied read and
  an unsupported call, because the tools show each case differently.

## Command-line parsing

- `pgrep`, `pkill` and `procps-helperd` use `clap` with derive and parse
  straight into typed values. procps-ng parses `pgrep` and `pkill` options with
  `getopt_long`, and `clap` accepts the same forms: clustered short options,
  attached values, abbreviated long options (`infer_long_args`) and options
  after the pattern.
- Before `clap` runs, `pkill` removes the first argument of the form `-<signal>`
  and uses it as the signal, as procps-ng's `signal_option` does. procps-ng
  removes only the first such argument, wherever it appears in the argument
  list. uutils/procps rewrites every such argument, which differs.
- `clap` errors and help output are rendered in procps-ng's format: glibc's
  `getopt` messages followed by procps-ng's usage text, with exit status 2.
- `ps` and `top` use parsers ported from procps-ng. `ps` mixes UNIX, BSD and GNU
  syntax in one command line (`ps aux`, `ps -ef`, `ps opid,cmd`), which `clap`
  cannot express. uutils/procps uses `clap` for `ps` and therefore rejects
  `ps aux`, `ps -u USER` and `--sort`; do not follow that approach. The ported
  parsers still produce typed option structs.

## Output

- Write through a locked, buffered handle to standard output. The `print_stdout`
  lint rejects `println!`.
- The tools restore the default `SIGPIPE` disposition at start-up. A closed pipe
  (`ps aux | head`) then terminates them silently, as it terminates procps-ng.
  Rust ignores `SIGPIPE` by default, and `println!` then panics on `EPIPE`.
- The helper client sets `SO_NOSIGPIPE` on its socket. If the helper closes the
  connection, the client gets an error and does not receive `SIGPIPE`.

## Observability

- Instrument the library crates with `tracing` spans, and use events for
  significant occurrences.
- The tools install a subscriber only when `PROCPS_DARWIN_LOG` is set, using it
  as an `EnvFilter` directive. The subscriber writes to standard error, in a
  pretty format on a terminal and as JSON otherwise. With the variable unset,
  the tools print only what procps-ng prints.
- `procps-helperd` logs JSON to standard error, which launchd captures.
- There is no OpenTelemetry exporter. Add one to the helper first if something
  starts collecting traces.

## Testing

- Use red/green TDD: write a failing test that describes the behaviour, then
  make it pass.
- Unit tests run against `FixtureSource`, never the live process table.
- Use `pretty_assertions::assert_eq!` for equality, `rstest` for parameterised
  cases, and `assert_matches!` for error variants and other enums. Do not use
  `unwrap()` in tests: use `?` or `expect()` with a useful message.
- Assert on whole values: whole records, whole slices, whole output strings.
- The procps-ng reference harness is the oracle for option parsing, error
  messages, exit codes and output layout. Add a harness scenario for each
  behaviour that procps-ng defines.

## Development environment

- `nix develop`, or direnv with the `.envrc`, provides the Rust toolchain,
  nightly `rustfmt` and `just`, and installs the git hooks. Run commands through
  it with `nix develop -c <command>`.
- `just fmt` formats every file with treefmt. `just check` runs
  `nix flake check`. When the Cargo workspace exists, the justfile gains
  `clippy`, `test` and `doc` recipes, and `just check` runs them too.
- The pre-commit hooks run treefmt, `nix flake check` and whitespace checks.
  wrapscallion checks commit messages: Conventional Commits, with bodies wrapped
  at 72 columns.
