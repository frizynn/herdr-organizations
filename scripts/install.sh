#!/bin/sh
# Builds target/release/herdr-organizations and the herdr-projects
# compatibility binary, then puts both commands on PATH.
#
# Herdr runs this as the plugin's build step, and `update` runs it in a linked
# checkout. Herdr Organizations publishes no prebuilt binaries (upstream's
# release assets are herdr-projects builds without the organization features),
# so this always builds from source: `cargo build --release --locked`.
# scripts/link-command.sh then links both commands.
set -u

cd "$(dirname "$0")/.." || exit 1

say() { printf 'herdr-organizations install: %s\n' "$*" >&2; }

if ! command -v cargo >/dev/null 2>&1; then
  say "cargo is not installed. Install Rust 1.89 or newer (https://rustup.rs) and a C compiler, then install again."
  exit 1
fi
say "building from source: cargo build --release --locked (this takes a minute or two)"
cargo build --release --locked || exit $?
for binary in herdr-organizations herdr-projects; do
  if [ ! -x "target/release/$binary" ]; then
    say "the build did not produce target/release/$binary"
    exit 1
  fi
done
sh scripts/link-command.sh
