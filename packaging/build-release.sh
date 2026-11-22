#!/usr/bin/env bash
# Runs inside the container of packaging/Containerfile (see scripts/release.sh): builds the Linux and Windows
# executables (the game and the mod kit) and packs them with the README and licence into dist/.
set -euo pipefail
version="$1"
name="deus-ex-aerugo-$version"
out=dist
tdir=target/container

cargo build --locked --profile dist -p dx-play -p aerugo-modkit --features dx-play/static-sdl,aerugo-modkit/static-sdl --target x86_64-unknown-linux-gnu --target-dir "$tdir"
cargo build --locked --profile dist -p dx-play -p aerugo-modkit --features dx-play/static-sdl,aerugo-modkit/static-sdl --target x86_64-pc-windows-gnu --target-dir "$tdir"

mkdir -p "$out"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

mkdir -p "$stage/linux/$name" "$stage/windows/$name"
cp "$tdir/x86_64-unknown-linux-gnu/dist/dx-play" "$stage/linux/$name/deusex"
cp "$tdir/x86_64-pc-windows-gnu/dist/dx-play.exe" "$stage/windows/$name/deusex.exe"
cp "$tdir/x86_64-unknown-linux-gnu/dist/deusex-modkit" "$stage/linux/$name/deusex-modkit"
cp "$tdir/x86_64-pc-windows-gnu/dist/deusex-modkit.exe" "$stage/windows/$name/deusex-modkit.exe"
for d in linux windows; do cp README.md LICENSE "$stage/$d/$name/"; done
tar -C "$stage/linux" -czf "$out/$name-linux-x86_64.tar.gz" "$name"
(cd "$stage/windows" && zip -qr - "$name") > "$out/$name-windows-x86_64.zip"
(cd "$out" && sha256sum "$name-linux-x86_64.tar.gz" "$name-windows-x86_64.zip" > "$name-SHA256SUMS.txt")
ls -l "$out/$name"*
