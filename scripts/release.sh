#!/usr/bin/env bash
# Builds the release archives for Linux and Windows in a container (Podman or Docker) -> dist/:
#   deus-ex-aerugo-<version>-linux-x86_64.tar.gz, deus-ex-aerugo-<version>-windows-x86_64.zip, ...-SHA256SUMS.txt
#
#   scripts/release.sh            the version in Cargo.toml
#   scripts/release.sh 0.1.1      another version name
#
# The image (packaging/Containerfile, Ubuntu 22.04 + Rust + MinGW-w64) is built on the first run.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
version="${1:-$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)}"
engine="${CONTAINER_ENGINE:-$(command -v podman || command -v docker || true)}"
[ -n "$engine" ] || { echo "needs podman or docker" >&2; exit 1; }
image=deus-ex-aerugo-build:ubuntu22.04

"$engine" build -t "$image" -f packaging/Containerfile packaging
mkdir -p target dist
# rootless Podman maps the caller to root in the container; Docker needs the caller's uid so the output is theirs
user=()
case "$(basename "$engine")" in docker) user=(--user "$(id -u):$(id -g)") ;; esac
# the build's tag or commit for the main menu (the container sees no git history: a worktree's .git points outside)
build="$(git -C "$root" describe --tags --exact-match 2>/dev/null || git -C "$root" rev-parse --short=6 HEAD 2>/dev/null || true)"
if [ -n "$build" ] && [ -n "$(git -C "$root" status --porcelain --untracked-files=no 2>/dev/null)" ]; then build="$build+"; fi
"$engine" run --rm "${user[@]}" -e AERUGO_BUILD="$build" -v "$root:/src:Z" "$image" bash packaging/build-release.sh "$version"
