#!/usr/bin/env bash
# Builds the Windows executable `deusex.exe` on Linux (cross-compiled with MinGW-w64) into ./dist.
# SDL3 is compiled from source and linked in, so the .exe needs nothing next to it but the game data.
#
#   scripts/build-windows.sh                    build only   -> dist/deusex.exe
#   scripts/build-windows.sh --install "<dir>"  build and copy it into a Deus Ex folder (the one with System/ and Maps/)
#
# Requirements: rustup (the script adds the x86_64-pc-windows-gnu target), the MinGW-w64 GCC cross compiler
# (`x86_64-w64-mingw32-gcc`), CMake (builds SDL3) and `glslc` or `glslangValidator` (shaders).
#   Arch:          pacman -S rustup mingw-w64-gcc cmake shaderc
#   Debian/Ubuntu: apt install gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64 cmake glslang-tools spirv-tools
# Or build both releases in a container instead: scripts/release.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

dir=""
while [ $# -gt 0 ]; do
    case "$1" in
        --install) dir="${2:-}"; [ -n "$dir" ] || { echo "--install needs the Deus Ex folder" >&2; exit 2; }; shift 2 ;;
        -h|--help) sed -n '2,14p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
    esac
done
if [ -n "$dir" ] && { [ ! -d "$dir/System" ] || [ ! -d "$dir/Maps" ]; }; then
    echo "'$dir' does not look like a Deus Ex folder (no System/ and Maps/)" >&2
    exit 2
fi

target=x86_64-pc-windows-gnu
missing=0
for tool in cargo rustup x86_64-w64-mingw32-gcc cmake; do
    command -v "$tool" >/dev/null 2>&1 || { echo "missing tool: $tool" >&2; missing=1; }
done
[ "$missing" = 0 ] || exit 1
rustup target list --installed | grep -qx "$target" || rustup target add "$target"

export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++
echo "== building for Windows ($target)"
cargo build --locked --profile dist -p dx-play -p aerugo-modkit --features dx-play/static-sdl,aerugo-modkit/static-sdl --target "$target"

mkdir -p dist
cp -f "target/$target/dist/dx-play.exe" dist/deusex.exe
cp -f "target/$target/dist/deusex-modkit.exe" dist/deusex-modkit.exe
echo "== built: $root/dist/deusex.exe ($(du -h dist/deusex.exe | cut -f1))"
if [ -n "$dir" ]; then
    cp -f dist/deusex.exe "$dir/deusex.exe"
    cp -f dist/deusex-modkit.exe "$dir/deusex-modkit.exe"
    echo "== installed: $dir/deusex.exe"
fi
