#!/usr/bin/env bash
# Writes THIRD_PARTY_NOTICES.md for what the native gems link: cargo about for
# the Rust crates, the Rust standard library's licenses at the pinned toolchain,
# and the licenses of the code inside rusty_v8's prebuilt archives, fetched at
# the commits rusty_v8 pins and recorded with digests in licenses/*/SOURCES.tsv.
#
# Both modes download any missing pinned archive and fail if one links source
# that licenses/v8/components.tsv does not list. --check fetches no licenses:
# it verifies licenses/ against the pins and the table, then diffs the file.
set -euo pipefail
cd "$(dirname "$0")/.."

check=false
case "${1:-}" in
  --check) check=true ;;
  "") ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

v8_version=$(awk '/^name = "v8"$/ { getline; gsub(/version = |"/, ""); print }' Cargo.lock)
rust_version=$(sed -nE 's/^channel = "([^"]+)"$/\1/p' rust-toolchain.toml)
table=licenses/v8/components.tsv
pins=ext/mini_racer_rs/v8-archives.sha256
cache="${MINI_RACER_RS_V8_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/mini_racer_rs/v8}/v$v8_version"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
  echo "VIOLATION [notices]: $*" >&2
  exit 1
}
digest() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | awk '{ print $1 }'
}
rows() {
  grep -v '^#' "$1" | grep -v '^$' || true
}

# The scan reads each archive's embedded source paths, resolves "..", and keys
# each by its first directory or by the third_party/<name> (or vendored Rust
# crate) it sits under, so a nested third_party never hides inside its parent.
mkdir -p "$cache"
archives=$(sed -nE 's/^[0-9a-f]{64}  (librusty_v8_release_.+\.a\.gz)$/\1/p' "$pins")
[ -n "$archives" ] || fail "$pins pins no archives"
known=$(rows "$table" | cut -f2 | sort -u)
for archive in $archives; do
  if [ ! -f "$cache/$archive" ]; then
    curl -fsSL --retry 3 -o "$cache/$archive.part" \
      "https://github.com/denoland/rusty_v8/releases/download/v$v8_version/$archive"
    mv "$cache/$archive.part" "$cache/$archive"
  fi
  [ "$(digest "$cache/$archive")" = "$(awk -v f="$archive" '$2 == f { print $1 }' "$pins")" ] ||
    fail "$archive does not match its pinned digest"
  linked=$(gunzip -c "$cache/$archive" | strings -n 6 | grep -oE '(\.\./)+[A-Za-z0-9_+./-]+/[A-Za-z0-9_+./-]+' |
    sed -E 's#^(\.\./)+##; s#/+#/#g' | awk -F/ '{
      split("", part)
      n = 0
      for (i = 1; i <= NF; i++) {
        if ($i == "..") { if (n > 0) n-- } else if ($i != "." && $i != "") { part[++n] = $i }
      }
      if (n < 2) next
      key = part[1]
      for (i = 1; i < n; i++) {
        if (part[i] == "third_party") {
          key = part[1]
          for (j = 2; j <= i + 1; j++) key = key "/" part[j]
          if (part[i + 1] == "rust" && part[i + 2] == "chromium_crates_io" && part[i + 3] == "vendor")
            key = key "/chromium_crates_io/vendor/" part[i + 4]
          break
        }
      }
      print key
    }' | sort -u)
  for dir in $linked; do
    grep -qxF "$dir" <<<"$known" || fail "$archive links $dir, which $table does not list"
  done
done

fetch() {
  local url="$1" dest="$2"
  mkdir -p "$(dirname "$dest")"
  case "$url" in
    https://chromium.googlesource.com/*) curl -fsSL --retry 3 "$url?format=TEXT" | base64 -d >"$dest" ;;
    *) curl -fsSL --retry 3 -o "$dest" "$url" ;;
  esac
}

if ! $check; then
  api=(curl -fsSL --retry 3 -H "Accept: application/vnd.github+json")
  token="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
  [ -n "$token" ] && api+=(-H "Authorization: Bearer $token")
  mkdir -p "$work/licenses/v8" "$work/licenses/rust"
  {
    printf '# rusty_v8 v%s\n' "$v8_version"
    printf '# component\tfile\tsource\tsha256\n'
  } >"$work/licenses/v8/SOURCES.tsv"
  while IFS=$'\t' read -r component _ source file; do
    [ "$file" = - ] && continue
    case "$source" in
      submodule:*)
        meta=$("${api[@]}" "https://api.github.com/repos/denoland/rusty_v8/contents/${source#submodule:}?ref=v$v8_version")
        base=$(jq -r '.submodule_git_url | sub("\\.git$"; "")' <<<"$meta")
        commit=$(jq -r .sha <<<"$meta")
        case "$base" in
          https://github.com/*) url="${base/github.com/raw.githubusercontent.com}/$commit/$file" ;;
          *) url="$base/+/$commit/$file" ;;
        esac
        ;;
      *\?*) url="${source%%\?*}/$file?${source#*\?}" ;;
      *) url="$source/$file" ;;
    esac
    fetch "$url" "$work/licenses/v8/$component/${file##*/}"
    printf '%s\t%s\t%s\t%s\n' "$component" "$file" "$url" "$(digest "$work/licenses/v8/$component/${file##*/}")" \
      >>"$work/licenses/v8/SOURCES.tsv"
  done < <(rows "$table")
  {
    printf '# rust %s\n' "$rust_version"
    printf '# component\tfile\tsource\tsha256\n'
  } >"$work/licenses/rust/SOURCES.tsv"
  for file in COPYRIGHT LICENSE-APACHE LICENSE-MIT; do
    url="https://raw.githubusercontent.com/rust-lang/rust/$rust_version/$file"
    fetch "$url" "$work/licenses/rust/std/$file"
    printf 'std\t%s\t%s\t%s\n' "$file" "$url" "$(digest "$work/licenses/rust/std/$file")" >>"$work/licenses/rust/SOURCES.tsv"
  done
  # The standard library's component notices are generated when Rust is built
  # and ship only with the toolchain, in the rustc component.
  case "$(rustc --version)" in
    "rustc $rust_version "*) ;;
    *) fail "rustc is $(rustc --version), not the pinned $rust_version" ;;
  esac
  cp "$(rustc --print sysroot)/share/doc/rust/COPYRIGHT-library.html" "$work/licenses/rust/std/"
  printf 'std\tCOPYRIGHT-library.html\trustc %s: share/doc/rust/COPYRIGHT-library.html\t%s\n' "$rust_version" \
    "$(digest "$work/licenses/rust/std/COPYRIGHT-library.html")" >>"$work/licenses/rust/SOURCES.tsv"
  cp "$table" "$work/licenses/v8/components.tsv"
fi

root=.
$check || root="$work"

verify() {
  local dir="$root/$1" header="$2" listed
  [ "$(head -1 "$dir/SOURCES.tsv")" = "$header" ] ||
    fail "$dir/SOURCES.tsv is for $(head -1 "$dir/SOURCES.tsv" | cut -c3-), expected ${header#\# }; run scripts/update-notices.sh"
  listed=$(rows "$dir/SOURCES.tsv" | while IFS=$'\t' read -r component file _ sha; do
    path="$dir/$component/${file##*/}"
    [ -f "$path" ] || fail "$path is listed in $dir/SOURCES.tsv but missing"
    [ "$(digest "$path")" = "$sha" ] || fail "$path does not match its digest in $dir/SOURCES.tsv"
    echo "$path"
  done)
  find "$dir" -mindepth 2 -type f | sort | while IFS= read -r path; do
    grep -qxF "$path" <<<"$listed" || fail "$path is not listed in $dir/SOURCES.tsv"
  done
}
verify licenses/v8 "# rusty_v8 v$v8_version"
verify licenses/rust "# rust $rust_version"
wanted=$(rows "$table" | awk -F'\t' '$4 != "-" { print $1 "\t" $4 }' | sort)
recorded=$(rows "$root/licenses/v8/SOURCES.tsv" | cut -f1,2 | sort)
[ "$wanted" = "$recorded" ] || fail "$table and licenses/v8/SOURCES.tsv list different files; run scripts/update-notices.sh"

render() {
  cargo about generate --locked about.hbs
  printf '## Rust standard library (rust %s)\n\n' "$rust_version"
  section licenses/rust
  printf '## V8 prebuilt archive (rusty_v8 v%s)\n\n' "$v8_version"
  section licenses/v8
}
# awk 1 ends every license with a newline, so its fence always closes. The
# HTML notices are too large to inline and ship beside this file instead.
section() {
  rows "$root/$1/SOURCES.tsv" | while IFS=$'\t' read -r component file source _; do
    printf '### %s\n\n%s\n\n' "$component" "$source"
    case "$file" in
      *.html) printf '`%s/%s/%s`\n\n' "$1" "$component" "${file##*/}" ;;
      *)
        printf '```text\n'
        awk 1 "$root/$1/$component/${file##*/}"
        printf '```\n\n'
        ;;
    esac
  done
}

render >"$work/THIRD_PARTY_NOTICES.md"
if $check; then
  cmp -s THIRD_PARTY_NOTICES.md "$work/THIRD_PARTY_NOTICES.md" ||
    fail "THIRD_PARTY_NOTICES.md is stale; run scripts/update-notices.sh"
  echo "Third-party notices are current."
else
  # Everything is staged and rendered; only now replace the tracked outputs.
  rm -rf licenses/v8 licenses/rust
  cp -R "$work/licenses/v8" "$work/licenses/rust" licenses/
  mv "$work/THIRD_PARTY_NOTICES.md" THIRD_PARTY_NOTICES.md
  echo "Wrote THIRD_PARTY_NOTICES.md and licenses/ for rusty_v8 v$v8_version and rust $rust_version."
fi
