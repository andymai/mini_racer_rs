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
  # Attributes span lines once rustfmt wraps a long reason, so each file is read
  # whole and every lint attribute naming unsafe_code is reported with the item
  # that follows it: "file:line:inner:item".
  while IFS=: read -r file line inner item; do
    module=$(sed -nE 's/^mod ([a-z_]+);$/\1/p' <<<"$item")
    if [ "$inner" = 1 ] || [ "$file" != "$path/src/lib.rs" ] || [ -z "$module" ] || ! grep -qw "$module" <<<"$allowed"; then
      echo "VIOLATION [fence]: $file:$line overrides unsafe_code outside the fenced modules ($allowed)"
      fail=1
    fi
  done < <(find "$path/src" "$path/build.rs" -name '*.rs' 2>/dev/null | sort | xargs perl -0777 -ne '
    while (/#(!?)\[\s*(?:allow|expect|warn)\s*\(([^\]]*)\]\s*([^\n]*)/g) {
      my ($inner, $body, $item, $at) = ($1, $2, $3, $-[0]);
      next unless $body =~ /\bunsafe_code\b/;
      my $line = 1 + (substr($_, 0, $at) =~ tr/\n//);
      printf "%s:%d:%d:%s\n", $ARGV, $line, ($inner eq "!" ? 1 : 0), $item;
    }
  ')
done < <(jq -r '.crates | to_entries[] | [.key, .value.path, ((.value.unsafe_modules // []) | join(" "))] | @tsv' <<<"$layers")

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "unsafe_code exemptions stay inside the fenced modules."
