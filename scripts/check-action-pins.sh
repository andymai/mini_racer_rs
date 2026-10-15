#!/usr/bin/env bash
# Every third-party action is pinned to a full commit SHA with a version comment.
set -euo pipefail
cd "$(dirname "$0")/.."

files=$(find .github -type f \( -name '*.yml' -o -name '*.yaml' \) 2>/dev/null || true)
[ -n "$files" ] || { echo "No workflows."; exit 0; }

hits=$(grep -nE '^\s*-?\s*uses:' $files |
  grep -vE 'uses:\s*\./' |
  grep -vE 'uses:\s*[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}\s+#\s*\S+' || true)
if [ -n "$hits" ]; then
  sed 's/^/VIOLATION [pin]: /' <<<"$hits"
  exit 1
fi
echo "Actions are SHA-pinned with version comments."
