#!/usr/bin/env bash
# Prints the digest-pinned image release/alpine/Dockerfile names for a stage:
# ruby-<minor> for a Ruby in tools/versions.toml [release] rubies, or floor.
# Usage: alpine-image.sh <stage>
set -euo pipefail
cd "$(dirname "$0")/.."

stage=${1:?usage: alpine-image.sh <stage>}
image=$(awk -v stage="$stage" '$1 == "FROM" && $3 == "AS" && $4 == stage { print $2 }' release/alpine/Dockerfile)
[ -n "$image" ] || { echo "alpine-image: release/alpine/Dockerfile has no stage $stage" >&2; exit 1; }
[[ "$image" =~ @sha256:[0-9a-f]{64}$ ]] || { echo "alpine-image: $image is not pinned to a digest" >&2; exit 1; }
echo "$image"
