#!/usr/bin/env bash
# Every third-party action is pinned to a full commit SHA with a version comment,
# and every container image a workflow runs to a digest, so a retag cannot swap
# either.
set -euo pipefail
cd "$(dirname "$0")/.."

mapfile -t files < <(find .github -type f \( -name '*.yml' -o -name '*.yaml' \) 2>/dev/null || true)
[ "${#files[@]}" -gt 0 ] || { echo "No workflows."; exit 0; }
fail=0

hits=$(grep -nE '^\s*-?\s*uses:' "${files[@]}" |
  grep -vE 'uses:\s*\./' |
  grep -vE 'uses:\s*[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}\s+#\s*\S+' || true)
if [ -n "$hits" ]; then
  sed 's/^/VIOLATION [pin]: /' <<<"$hits"
  fail=1
fi

# A container: or image: value that is an expression names a matrix or env
# value, whose own key line is checked. A docker run is read with its
# continuation lines joined, and may take its image from
# scripts/alpine-image.sh, which reads the Dockerfile FROM lines checked last.
digest='@sha256:[0-9a-f]{64}'
keys=$(grep -nE '^\s*-?\s*(container|image):\s*[^[:space:]#]' "${files[@]}" |
  grep -vE ':\s*(container|image):\s*\$\{\{' | grep -vE "$digest" || true)
runs=$(awk '{ line = line $0 } /\\$/ { sub(/\\$/, " ", line); next } { print FILENAME ":" FNR ": " line; line = "" }' "${files[@]}" |
  grep -E '(docker|podman) +(container +)?run( |$)' |
  grep -vE "$digest" | grep -vF "\$(scripts/alpine-image.sh " || true)
mapfile -t dockerfiles < <(find . \( -name .git -o -name node_modules -o -path ./vendor -o -path ./target -o -path ./tmp \) -prune -o \
  -type f \( -name 'Dockerfile*' -o -name '*.Dockerfile' -o -name 'Containerfile*' \) -print)
froms=$(for dockerfile in "${dockerfiles[@]}"; do
  grep -nHE '^\s*FROM\s' "$dockerfile" | grep -vE "FROM\s+\S+$digest(\s|$)" || true
done)
images=$(printf '%s\n' "$keys" "$runs" "$froms" | sed '/^$/d')
if [ -n "$images" ]; then
  sed 's/^/VIOLATION [image]: /' <<<"$images"
  fail=1
fi

[ "$fail" -eq 0 ] || exit 1
echo "Actions are SHA-pinned with version comments, and container images to digests."
