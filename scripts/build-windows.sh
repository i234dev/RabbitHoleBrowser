#!/bin/sh
# Cross-compile the Windows x64 binary from Linux without a mingw toolchain,
# using zig as the C compiler/linker (see scripts/dev-env.sh for the Linux
# setup). Requires the wrappers in ~/.local/bin: zigcc-win (zig cc
# -target x86_64-windows-gnu, filtering mingw-only flags and swapping in a
# compiler_builtins rlib without ___chkstk_ms), ar (zig ar) and
# x86_64-w64-mingw32-dlltool (zig dlltool).
set -e
cd "$(dirname "$0")/.."
. scripts/dev-env.sh
rustup target add x86_64-pc-windows-gnu >/dev/null
CC_x86_64_pc_windows_gnu=zigcc-win AR_x86_64_pc_windows_gnu=ar \
  cargo build --release -p rabbithole --target x86_64-pc-windows-gnu
ls -la target/x86_64-pc-windows-gnu/release/rabbithole.exe
