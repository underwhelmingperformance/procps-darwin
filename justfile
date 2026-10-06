# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later

fmt:
    nix fmt

build:
    cargo build --locked --workspace

clippy:
    cargo clippy --locked --workspace --all-targets -- --deny warnings

test:
    cargo test --locked --workspace

doc:
    RUSTDOCFLAGS="--deny warnings" cargo doc --locked --workspace --no-deps

deny:
    cargo deny check bans licenses sources

audit:
    cargo audit

reuse:
    reuse lint

check: clippy test doc deny audit reuse
    nix flake check
