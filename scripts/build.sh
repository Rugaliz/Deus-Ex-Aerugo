#!/usr/bin/env bash
# Builds the game executable `deusex` and the mod kit `deusex-modkit` (optimised) into ./dist and, on request, drops
# them into your Deus Ex folder. Every run is a clean build: earlier build output (target/ and dist/) is removed first.
#
#   scripts/build.sh                      build only          -> dist/deusex
#   scripts/build.sh --install "<dir>"    build and copy it into the Deus Ex folder (the one with System/ and Maps/)
#   scripts/build.sh --run "<dir>"        build, then start it from that folder
#   scripts/build.sh --test "<dir>"       build, then run the test suite against that folder first
#
# `deusex` needs only the original game data next to it. Run it from the game folder (no arguments: it opens on the main
# menu), or from anywhere with the folder as an argument: `deusex "<dir>" [MapName]`. Settings and key bindings are kept
# in ~/.config/deus-ex-aerugo (nothing is written into the game folder except the executable you ask to install).
#
# Build requirements (see README.md for the Arch and Ubuntu package lists): a Rust toolchain, a shader compiler
# (`glslc` or `glslangValidator`), and SDL3. When the system has no SDL3 development files (Ubuntu 24.04 and older),
# SDL3 is compiled from source and built into the executable; that needs CMake and the X11/Wayland/audio headers.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

mode=build
dir=""
while [ $# -gt 0 ]; do
    case "$1" in
        --install|--run|--test)
            mode="${1#--}"
            dir="${2:-}"
            [ -n "$dir" ] || { echo "$1 needs the Deus Ex folder" >&2; exit 2; }
            shift 2 ;;
        -h|--help) sed -n '2,16p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
    esac
done

if [ -n "$dir" ] && { [ ! -d "$dir/System" ] || [ ! -d "$dir/Maps" ]; }; then
    echo "'$dir' does not look like a Deus Ex folder (no System/ and Maps/)" >&2
    exit 2
fi

missing=0
for tool in cargo; do
    command -v "$tool" >/dev/null 2>&1 || { echo "missing tool: $tool" >&2; missing=1; }
done
command -v glslc >/dev/null 2>&1 || command -v glslangValidator >/dev/null 2>&1 || { echo "missing tool: glslc or glslangValidator" >&2; missing=1; }
# the system's SDL3 when its development files are there, else SDL3 built from source into the executable
features=()
if pkg-config --exists sdl3 2>/dev/null || ldconfig -p 2>/dev/null | grep -q 'libSDL3\.so '; then
    echo "== using the system's SDL3"
else
    echo "== no SDL3 development files found: SDL3 is built from source into the executable"
    command -v cmake >/dev/null 2>&1 || { echo "missing tool: cmake (needed to build SDL3)" >&2; missing=1; }
    features=(--features dx-play/static-sdl,aerugo-modkit/static-sdl)
fi
[ "$missing" = 0 ] || exit 1

echo "== cleaning earlier build output"
cargo clean
rm -f dist/deusex dist/deusex-modkit

if [ "$mode" = test ]; then
    echo "== tests (against $dir)"
    DEUSEX_DIR="$dir" cargo test --release --no-fail-fast
fi

echo "== building (dist profile: optimised, no debug info)"
cargo build --locked --profile dist -p dx-play -p aerugo-modkit "${features[@]}"

mkdir -p dist
cp -f target/dist/dx-play dist/deusex
cp -f target/dist/deusex-modkit dist/deusex-modkit
echo "== built: $root/dist/deusex ($(du -h dist/deusex | cut -f1)), $root/dist/deusex-modkit ($(du -h dist/deusex-modkit | cut -f1))"

case "$mode" in
    install)
        cp -f dist/deusex "$dir/deusex"
        cp -f dist/deusex-modkit "$dir/deusex-modkit"
        echo "== installed: $dir/deusex   (run it from that folder: ./deusex)" ;;
    run)
        echo "== starting from $dir"
        cd "$dir" && exec "$root/dist/deusex" ;;
esac
