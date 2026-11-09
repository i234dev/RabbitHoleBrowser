# Source this to build on a machine without a system C toolchain
# (uses rustup's cargo and zig as the C compiler/linker/archiver).
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CC=zigcc
export AR=ar
