#!/usr/bin/env bash
# Enforces layers.toml against the resolved cargo graph. Every rule prints a
# tagged VIOLATION line; scripts/test-boundaries.sh plants one per tag.
set -euo pipefail
cd "$(dirname "$0")/.."

layers=$(taplo get -f layers.toml -o json)
meta=$(mktemp)
violations=$(mktemp)
trap 'rm -f "$meta" "$violations"' EXIT
cargo metadata --format-version 1 >"$meta"

# Each report runs on the right of a pipe, in a subshell, so findings go to a
# file rather than a shell variable.
report() {
  local tag="$1"
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    echo "VIOLATION [$tag]: $line" >>"$violations"
  done
}

jq -r --argjson layers "$layers" '. as $meta |
  ([$meta.workspace_members[] as $id | $meta.packages[] | select(.id == $id) | .name] | sort) as $members
  | ($layers.crates | keys | sort) as $declared
  | ($members - $declared | map("workspace member \(.) is missing from layers.toml")[]),
    ($declared - $members | map("layers.toml crate \(.) is not a workspace member")[])
' "$meta" | report members

jq -r --argjson layers "$layers" '. as $meta |
  $meta.workspace_members as $ids
  | $meta.packages[] | select(.id as $id | $ids | index($id)) as $pkg
  | ($layers.crates[$pkg.name] // empty) as $rule
  | $pkg.dependencies[] | select(.kind == null)
  | (if .target then ($rule.target_normal[.target] // []) else [] end) as $scoped
  | select(.name as $dep | $rule.normal + $scoped | index($dep) | not)
  | "\($pkg.name) depends on \(.name)\(if .target then " under \(.target)" else "" end), which layers.toml does not allow"
' "$meta" | report deps

jq -r --argjson layers "$layers" '. as $meta |
  $meta.workspace_members as $ids
  | $meta.packages[] | select(.id as $id | $ids | index($id)) as $pkg
  | ($layers.crates[$pkg.name].layer // empty) as $own
  | $pkg.dependencies[] | select(.kind == "dev" or .kind == "build")
  | ($layers.crates[.name].layer // empty) as $dep
  | select($dep > $own)
  | "\($pkg.name) (L\($own)) has a \(.kind) dependency on \(.name) (L\($dep))"
' "$meta" | report dev-deps

jq -r --argjson layers "$layers" '. as $meta |
  ($meta.packages | map({key: .id, value: .name}) | from_entries) as $names
  | ($meta.resolve.nodes
      | map({key: .id, value: [.deps[] | select(any(.dep_kinds[]; .kind == null)) | .pkg]})
      | from_entries) as $graph
  | $meta.workspace_members[] as $root
  | $names[$root] as $crate
  | select(($layers.crates[$crate].layer // 99) < 2)
  | {todo: $graph[$root], seen: {}}
  | until(.todo | length == 0;
      .todo[0] as $n
      | .todo |= .[1:]
      | if .seen[$n] then . else .seen[$n] = true | .todo += ($graph[$n] // []) end)
  | .seen | keys[] | $names[.]
  | select(. as $dep | $layers.rules.ruby_only | index($dep))
  | "\($crate) reaches \(.) through its normal dependencies"
' "$meta" | sort -u | report transitive

jq -r --argjson layers "$layers" '. as $meta |
  $meta.workspace_members as $ids
  | $meta.packages[] | select(.id as $id | $ids | index($id)) as $pkg
  | select($pkg.name != "mini_racer_engine")
  | $pkg.dependencies[] | select(.name as $dep | $layers.rules.engine_only | index($dep))
  | "\($pkg.name) names \(.name) directly; only mini_racer_engine may"
' "$meta" | report engine-only

jq -r --argjson layers "$layers" '. as $meta |
  ($meta.packages | map(select(.name == "v8")) | map(.id)) as $v8
  | $meta.resolve.nodes[] | select(.id as $id | $v8 | index($id))
  | .features[] | select(. as $f | $layers.rules.v8_forbidden_features | index($f))
  | "v8 feature \(.) is enabled"
' "$meta" | report v8-features

jq -r --argjson layers "$layers" '. as $meta |
  $meta.workspace_members as $ids
  | $meta.packages[] | select(.id as $id | $ids | index($id))
  | select(any(.targets[]; .crate_types | index("cdylib")) != ($layers.crates[.name].cdylib // false))
  | "\(.name) cdylib status does not match layers.toml"
' "$meta" | report cdylib

jq -r '. as $meta |
  $meta.workspace_members as $ids
  | $meta.packages[] | select(.id as $id | $ids | index($id))
  | select(.publish != [])
  | "\(.name) is publishable; every workspace crate sets publish = false"
' "$meta" | report publish

while IFS=$'\t' read -r crate path; do
  if ! grep -qx '#!\[forbid(unsafe_code)\]' "$path/src/lib.rs"; then
    echo "$crate is unsafe = \"forbid\" but $path/src/lib.rs lacks #![forbid(unsafe_code)]"
  fi
done < <(jq -r '.crates | to_entries[] | select(.value.unsafe == "forbid") | [.key, .value.path] | @tsv' <<<"$layers") |
  report forbid

if [ -f CLAUDE.md ]; then
  expected=$(jq -r '.crates | to_entries | sort_by(.value.layer, .key)[] | "| `\(.key)` | L\(.value.layer) |"' <<<"$layers")
  actual=$(grep -E '^\| `mini_racer_[a-z]+` \| L[0-9] \|' CLAUDE.md | sed -E 's/^(\| `[a-z_]+` \| L[0-9] \|).*/\1/' || true)
  if [ "$expected" != "$actual" ]; then
    echo "CLAUDE.md layer table does not match layers.toml" | report claude-md
  fi
fi

if [ -s "$violations" ]; then
  cat "$violations"
  exit 1
fi
echo "Crate boundaries match layers.toml."
