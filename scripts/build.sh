#!/usr/bin/env bash
# Builds the game executable `deusex` (optimised) into ./dist and, on request, drops it into your Deus Ex folder.
# Every run is a clean build: earlier build output (target/ and dist/deusex) is removed first.
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
# Build requirements (Arch: pacman -S rustup sdl3 vulkan-icd-loader shaderc; then `rustup default stable`):
#   a Rust toolchain, `glslc` (compiles the shaders into the binary), SDL3 (the library it links to).
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
libs="$(ldconfig -p 2>/dev/null || true)"
for lib in libSDL3.so; do
    case "$libs" in *"$lib"*) ;; *) echo "warning: $lib not found by ldconfig (the build or the first run may fail)" >&2 ;; esac
done
[ "$missing" = 0 ] || exit 1

echo "== cleaning earlier build output"
cargo clean
rm -f dist/deusex

if [ "$mode" = test ]; then
    echo "== tests (against $dir)"
    DEUSEX_DIR="$dir" cargo test --release --no-fail-fast
fi

echo "== building (dist profile: optimised, no debug info)"
cargo build --locked --profile dist -p dx-play

mkdir -p dist
cp -f target/dist/dx-play dist/deusex
echo "== built: $root/dist/deusex ($(du -h dist/deusex | cut -f1))"

case "$mode" in
    install)
        cp -f dist/deusex "$dir/deusex"
        echo "== installed: $dir/deusex   (run it from that folder: ./deusex)" ;;
    run)
        echo "== starting from $dir"
        cd "$dir" && exec "$root/dist/deusex" ;;
esac
