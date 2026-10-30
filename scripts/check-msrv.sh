#!/bin/sh
# Check that the library, its tests and examples build warning-free on the crate's rust-version
# (Cargo.toml), with and without the serde feature. Needs that toolchain:
# `rustup toolchain install 1.85`. The fuzz crate is nightly-only and not checked here.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
msrv=$(sed -n 's/^rust-version *= *"\(.*\)"/\1/p' Cargo.toml)
[ -n "$msrv" ] || { echo "no rust-version in Cargo.toml" >&2; exit 1; }
export RUSTFLAGS=-Dwarnings
cargo "+$msrv" test --no-run --all-targets --locked
cargo "+$msrv" test --no-run --all-targets --locked --features serde
echo "builds on Rust $msrv"
