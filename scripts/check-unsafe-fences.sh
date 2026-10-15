#!/usr/bin/env bash
# `unsafe_code` is denied workspace-wide. The only exemptions are the module
# declarations layers.toml lists under unsafe_modules, so this checks that every
# lint override names one of them and that no crate opts out of the workspace lints.
set -euo pipefail
cd "$(dirname "$0")/.."

layers=$(taplo get -f layers.toml -o json)
fail=0

while IFS=$'\t' read -r crate path allowed; do
  if ! awk '/^\[lints\]$/ { getline; if ($0 == "workspace = true") found = 1 } END { exit !found }' "$path/Cargo.toml"; then
    echo "VIOLATION [lints]: $crate does not inherit the workspace lints"
    fail=1
  fi
  while IFS= read -r hit; do
    file=${hit%%:*}
    rest=${hit#*:}
    line=${rest%%:*}
    next=$(sed -n "$((line + 1))p" "$file")
    module=$(sed -nE 's/^mod ([a-z_]+);$/\1/p' <<<"$next")
    if [ "$file" != "$path/src/lib.rs" ] || [ -z "$module" ] || ! grep -qw "$module" <<<"$allowed"; then
      echo "VIOLATION [fence]: $file:$line overrides unsafe_code outside the fenced modules ($allowed)"
      fail=1
    fi
  done < <(grep -rnE '(allow|expect|warn)\(\s*unsafe_code' "$path/src" "$path/build.rs" 2>/dev/null || true)
done < <(jq -r '.crates | to_entries[] | [.key, .value.path, ((.value.unsafe_modules // []) | join(" "))] | @tsv' <<<"$layers")

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "unsafe_code exemptions stay inside the fenced modules."
