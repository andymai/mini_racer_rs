# Sourced, not executed: downloads the pinned rusty_v8 archive and source binding
# for this host once, verifies them against ext/mini_racer_rs/v8-archives.sha256,
# and exports the RUSTY_V8_* variables so cargo never fetches them itself.
# Sourced from bash (gate.sh, CI) and POSIX sh (husky hooks), so no bashisms.
# shellcheck shell=sh

mini_racer_rs_fetch_v8() {
  local root version target base cache file sha actual
  root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
  version=$(awk '/^name = "v8"$/ { getline; gsub(/version = |"/, ""); print }' "$root/Cargo.lock")
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
    Linux-aarch64) target=aarch64-unknown-linux-gnu ;;
    Darwin-arm64) target=aarch64-apple-darwin ;;
    Darwin-x86_64) target=x86_64-apple-darwin ;;
    *) echo "fetch-v8-archive: unsupported host $(uname -s)-$(uname -m)" >&2; return 1 ;;
  esac
  base="https://github.com/denoland/rusty_v8/releases/download/v$version"
  cache="${MINI_RACER_RS_V8_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/mini_racer_rs/v8}/v$version"
  mkdir -p "$cache"
  for file in "librusty_v8_release_$target.a.gz" "src_binding_release_$target.rs"; do
    sha=$(awk -v f="$file" '$2 == f { print $1 }' "$root/ext/mini_racer_rs/v8-archives.sha256")
    [ -n "$sha" ] || { echo "fetch-v8-archive: no pinned digest for $file" >&2; return 1; }
    if [ ! -f "$cache/$file" ]; then
      curl -fsSL --retry 3 -o "$cache/$file.part" "$base/$file" && mv "$cache/$file.part" "$cache/$file" || return 1
    fi
    if command -v sha256sum >/dev/null; then
      actual=$(sha256sum "$cache/$file" | awk '{ print $1 }')
    else
      actual=$(shasum -a 256 "$cache/$file" | awk '{ print $1 }')
    fi
    if [ "$actual" != "$sha" ]; then
      echo "fetch-v8-archive: $file digest $actual does not match the pinned $sha" >&2
      rm -f "$cache/$file"
      return 1
    fi
  done
  export RUSTY_V8_ARCHIVE="$cache/librusty_v8_release_$target.a.gz"
  export RUSTY_V8_SRC_BINDING_PATH="$cache/src_binding_release_$target.rs"
  export RUSTY_V8_ARCHIVE_SHA256
  RUSTY_V8_ARCHIVE_SHA256=$(awk -v f="librusty_v8_release_$target.a.gz" '$2 == f { print $1 }' "$root/ext/mini_racer_rs/v8-archives.sha256")
}

mini_racer_rs_fetch_v8
