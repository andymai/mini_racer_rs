#!/usr/bin/env bash
# Mutation testing with cargo-mutants into tmp/mutants/<run>/mutants.out:
# under CI in place, reusing the cached build, and otherwise in a copy.
#   wire [k/n]       mini_racer_wire against its tests and mini_racer_engine's,
#                    which write and read V8's own bytes through it; shard k of
#                    n, counting from 0
#   sync             mini_racer_sync against its tests
#   loom             the mutants of mini_racer_sync its tests missed, against
#                    the loom models (reads tmp/mutants/sync)
#   report <dir>...  a Markdown table of finished runs and the mutants that
#                    survived them
# Surviving mutants and timeouts are results, not failures: a run fails only
# when cargo-mutants cannot run or the unmutated tests fail.
set -euo pipefail
cd "$(dirname "$0")/.."

mode=${1:-}
[ "$#" -gt 0 ] && shift
# Otherwise proptest saves a mutant's failing cases into the tree, and the
# mutants after it replay them.
export PROPTEST_DISABLE_FAILURE_PERSISTENCE=1

mutants() {
  local run=$1 status=0
  shift
  mkdir -p tmp/mutants
  cargo mutants --output "tmp/mutants/$run" --cargo-arg=--locked ${CI:+--in-place} "$@" || status=$?
  case "$status" in
    0 | 2 | 3) ;;
    *) echo "mutants: cargo mutants exited $status" >&2; exit "$status" ;;
  esac
}

label() {
  case "$(basename "$1")" in
    *wire*) printf 'mini_racer_wire\t%s' "its tests and the engine's" ;;
    *loom*) printf 'mini_racer_sync\t%s' "the loom models, after its tests" ;;
    *sync*) printf 'mini_racer_sync\t%s' "its tests" ;;
    *) printf '%s\t%s' "$(basename "$1")" "unknown" ;;
  esac
}

report() {
  local dir other labels="" name missed
  local runs=()
  for dir in "$@"; do
    if [ -f "$dir/mutants.out/outcomes.json" ]; then
      runs+=("$dir")
    else
      echo "mutants: no results in $dir" >&2
    fi
  done
  echo "| Crate | Tested by | Mutants | Caught | Missed | Timeouts | Unviable |"
  echo "|---|---|--:|--:|--:|--:|--:|"
  [ "${#runs[@]}" -gt 0 ] || return 0
  for dir in "${runs[@]}"; do
    printf '%s\t%s\n' "$(label "$dir")" \
      "$(jq -r '[.total_mutants, .caught, .missed, .timeout, .unviable] | @tsv' "$dir/mutants.out/outcomes.json")"
  done | awk -F '\t' '
    { key = $1 " | " $2
      if (!(key in total)) order[n++] = key
      total[key] += $3; caught[key] += $4; missed[key] += $5; timeout[key] += $6; unviable[key] += $7 }
    END { for (i = 0; i < n; i++) { k = order[i]
      printf "| %s | %d | %d | %d | %d | %d |\n", k, total[k], caught[k], missed[k], timeout[k], unviable[k] } }'
  # Shards of one run share a label, so their survivors are listed together.
  for dir in "${runs[@]}"; do
    name=$(label "$dir")
    case "$labels" in *"[$name]"*) continue ;; esac
    labels+="[$name]"
    missed=$(for other in "${runs[@]}"; do
      if [ "$(label "$other")" = "$name" ]; then cat "$other/mutants.out/missed.txt"; fi
    done | sort -t: -k1,1 -k2,2n -k3,3n)
    [ -n "$missed" ] || continue
    # shellcheck disable=SC2016
    printf '\n<details><summary>%s, missed by %s: %d</summary>\n\n```text\n%s\n```\n\n</details>\n' \
      "${name%%$'\t'*}" "${name#*$'\t'}" "$(wc -l <<<"$missed")" "$missed"
  done
}

case "$mode" in
  wire)
    # cargo-mutants' own baseline runs only the mutated crate's tests, so both
    # crates' run here unmutated instead, and their time sets the timeout.
    cargo nextest run --no-run -p mini_racer_wire -p mini_racer_engine --locked
    start=$SECONDS
    cargo nextest run -p mini_racer_wire -p mini_racer_engine --locked
    timeout=$((5 * (SECONDS - start)))
    [ "$timeout" -ge 60 ] || timeout=60
    run=wire
    shard=()
    if [ -n "${1:-}" ]; then
      run="wire-${1%/*}"
      shard=(--shard "$1" --sharding round-robin)
    fi
    mutants "$run" -p mini_racer_wire --test-package mini_racer_wire,mini_racer_engine \
      --test-tool nextest --baseline skip --timeout "$timeout" "${shard[@]}"
    ;;
  sync)
    mutants sync -p mini_racer_sync --test-tool nextest
    ;;
  loom)
    unit=tmp/mutants/sync/mutants.out
    [ -f "$unit/outcomes.json" ] || { echo "mutants: run scripts/mutants.sh sync first" >&2; exit 1; }
    # --iterate skips the mutants a mutants.out already in the output
    # directory lists as caught, so the unit tests' kills and timeouts go there.
    rm -rf tmp/mutants/loom
    mkdir -p tmp/mutants/loom/mutants.out
    for list in caught timeout unviable; do
      if [ -f "$unit/$list.txt" ]; then cat "$unit/$list.txt"; fi
    done >tmp/mutants/loom/mutants.out/caught.txt
    RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--cfg loom" CARGO_TARGET_DIR=target/loom \
      mutants loom -p mini_racer_sync --iterate --profile release --cargo-arg=--test=loom
    ;;
  report)
    report "$@"
    ;;
  *)
    echo "usage: $0 wire [k/n] | sync | loom | report <dir>..." >&2
    exit 2
    ;;
esac
