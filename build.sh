#!/usr/bin/env bash
# Release build: cross-compile the Windows exes from a Linux host (WSL2 works).
# Requires: rustup target x86_64-pc-windows-gnu, mingw-w64 (x86_64-w64-mingw32-gcc).
# Override CARGO_TARGET_DIR to keep build output off a synced folder.
set -euo pipefail
cd "$(dirname "$0")"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_AR=x86_64-w64-mingw32-ar
cargo build --release --target x86_64-pc-windows-gnu 2>&1 | grep -E '^(error|warning)|-->|Finished|^\s+\|' | head -60
mkdir -p dist
for b in mw5save MW5SaveEditor; do
  cp "$CARGO_TARGET_DIR/x86_64-pc-windows-gnu/release/$b.exe" "dist/$b.exe"
done
cp dist/mw5save.exe ./mw5save.exe
ls -la dist/*.exe
sha256sum dist/*.exe | tee dist/SHA256SUMS.txt
