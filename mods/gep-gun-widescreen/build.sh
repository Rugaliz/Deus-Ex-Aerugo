#!/usr/bin/env bash
# Builds the "GEP gun widescreen" mesh mod from your own game data (nothing of the game is stored in this repository).
#
#   mods/gep-gun-widescreen/build.sh "<Deus Ex folder>" [<output folder>]
#
# Needs `deusex-modkit` (on PATH, next to this repository's dist/, or in target/release) and Blender 4 or later.
# The result: <output folder>/GEPGunWidescreen.zip (default output folder: ./ModKit). Put it into <Deus Ex>/Mods/.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
game="${1:?the Deus Ex folder}"
out="$(mkdir -p "${2:-ModKit}" && cd "${2:-ModKit}" && pwd)"
kit="$(command -v deusex-modkit || true)"
for k in "$root/dist/deusex-modkit" "$root/target/release/deusex-modkit"; do [ -n "$kit" ] || { [ -x "$k" ] && kit="$k"; }; done
[ -n "$kit" ] || { echo "deusex-modkit not found (build it: cargo build --release -p aerugo-modkit)" >&2; exit 1; }
command -v blender >/dev/null || { echo "Blender not found" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# the game's model, as glTF, and its vertex positions as the script finds them
gltf="$("$kit" --game "$game" export DeusExItems.GEPGun --out "$work/export")"
"$kit" --game "$game" export-obj DeusExItems.GEPGun --stored --frame 0 -o "$work/frame0.obj" >/dev/null
# the edit, made in Blender as an artist would (see close_rear.py)
blender -b --python "$here/close_rear.py" -- "$gltf" "$work/frame0.obj" "$work/GEPGun.glb" | grep -E "close_rear|Error" || true
# back into a mesh mod
mod="$work/GEPGunWidescreen"
"$kit" --game "$game" import "$work/GEPGun.glb" --mod "$mod" --mesh DeusExItems.GEPGun
cp "$here/README.md" "$mod/README.md"
rm -f "$out/GEPGunWidescreen.zip"
(cd "$work" && zip -qr "$out/GEPGunWidescreen.zip" GEPGunWidescreen)
echo "== $out/GEPGunWidescreen.zip: put it into <Deus Ex>/Mods/"
