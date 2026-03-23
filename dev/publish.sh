#!/usr/bin/env bash
set -euo pipefail

# Publish all crates in dependency order.
# cargo publish waits for the crate to be available on crates.io before returning.

echo "Publishing migratio..."
cargo publish -p migratio

echo "Publishing migratio-cli..."
cargo publish -p migratio-cli

echo "Publishing cargo-migratio..."
cargo publish -p cargo-migratio

echo "All crates published."
