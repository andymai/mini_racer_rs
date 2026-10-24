#!/usr/bin/env bash
# Writes a release's SBOMs into a directory: mini_racer_rs-<version>.cdx.json
# (CycloneDX 1.5) and mini_racer_rs-<version>.spdx.json (SPDX 2.3). Both list
# the Rust crates the extension links on each target a V8 archive is pinned
# for (cargo-cyclonedx, at the version tools/versions.toml pins), each of those
# archives with its pinned digest, and each source licenses/v8/components.tsv
# says they compile, with the license URLs licenses/v8/SOURCES.tsv took. The
# dependency graph is the union over the targets, so the v8 crate depends on
# every archive and every source; the table does not say which archive
# compiles which source. The SPDX document is mapped from the CycloneDX one
# here, since the converters tried drop data: syft 1.54.1 every checksum,
# cyclonedx-cli 0.33.1 every dependency, and the latter writes SPDX ids the
# SPDX grammar refuses.
# Usage: sbom.sh <dir>
set -euo pipefail
cd "$(dirname "$0")/.."

out=${1:?usage: sbom.sh <dir>}
version=$(sed -nE 's/^ *VERSION = "([^"]+)"$/\1/p' lib/mini_racer_rs/version.rb)
v8=$(awk '/^name = "v8"$/ { getline; gsub(/version = |"/, ""); print }' Cargo.lock)
pins=ext/mini_racer_rs/v8-archives.sha256
targets=$(sed -nE 's/^[0-9a-f]{64}  librusty_v8_release_(.+)\.a\.gz$/\1/p' "$pins")
[ -n "$targets" ] || { echo "sbom: $pins pins no archives" >&2; exit 1; }
# The commit time, as packaging uses, so a commit's SBOMs are the same bytes
# wherever they are made.
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}
export SOURCE_DATE_EPOCH
work=$(mktemp -d)
# cargo-cyclonedx writes a document beside every workspace member's manifest,
# under a name of this run's own.
name="sbom-$$"
trap 'rm -rf "$work"; rm -f crates/*/"$name"-*.json ext/*/"$name"-*.json' EXIT
mkdir -p "$out"

for target in $targets; do
  cargo cyclonedx --manifest-path ext/mini_racer_rs/Cargo.toml --format json --spec-version 1.5 \
    --target "$target" --no-build-deps --override-filename "$name-$target" -q
  mv "ext/mini_racer_rs/$name-$target.json" "$work/$target.json"
done

tsv() { grep -v '^#' "$1" | grep -v '^$' || true; }
cdx="$out/mini_racer_rs-$version.cdx.json"
# Paths in cargo's package ids become /mini_racer_rs, as the release remaps
# them in the extension.
jq -s --arg root "$PWD" --arg v8 "$v8" \
  --rawfile pins "$pins" --rawfile table <(tsv licenses/v8/components.tsv) --rawfile sources <(tsv licenses/v8/SOURCES.tsv) '
  def lines($text): $text | split("\n") | map(select(. != ""));
  def ref_v8: "registry+https://github.com/rust-lang/crates.io-index#v8@\($v8)";
  ([lines($pins)[] | capture("^(?<sha>[0-9a-f]{64})  (?<file>librusty_v8_release_(?<target>.+)\\.a\\.gz)$")]) as $archives
  | ([lines($table)[] | split("\t") | select(.[2] != "-") | {name: .[0], path: .[1]}]
      | group_by(.name) | map({name: .[0].name, paths: (map(.path) | unique)})) as $compiled
  | ([lines($sources)[] | split("\t") | {name: .[0], url: .[2]}]) as $licenses
  | ($compiled | map({
      type: "library",
      "bom-ref": "rusty_v8-source:\(.name)",
      name,
      externalReferences: [.name as $n | $licenses[] | select(.name == $n) | {type: "license", url}],
      properties: [.paths[] | {name: "mini_racer_rs:archive_path", value: .}]
    })) as $source_components
  | ($archives | map({
      type: "library",
      "bom-ref": "rusty_v8-archive:\(.target)",
      name: .file,
      version: $v8,
      hashes: [{alg: "SHA-256", content: .sha}],
      purl: "pkg:generic/\(.file)@\($v8)?download_url=https://github.com/denoland/rusty_v8/releases/download/v\($v8)/\(.file)&checksum=sha256:\(.sha)",
      externalReferences: [{type: "distribution", url: "https://github.com/denoland/rusty_v8/releases/download/v\($v8)/\(.file)"}],
      properties: [{name: "mini_racer_rs:target", value: .target}]
    })) as $archive_components
  | .[0] + {
      metadata: (.[0].metadata + {properties: [$archives[] | {name: "cdx:rustc:sbom:target:triple", value: .target}]}),
      components: ((map(.components[]) | unique_by(."bom-ref")) + $archive_components + $source_components),
      dependencies: (
        (map(.dependencies[]) | group_by(.ref) | map({ref: .[0].ref, dependsOn: (map(.dependsOn // []) | add | unique)}))
        | map(if .ref == ref_v8 then .dependsOn += [($archive_components + $source_components)[] | ."bom-ref"] else . end)
      )
    }
  | if any(.dependencies[]; .ref == ref_v8) then . else error("no v8 \($v8) in the crate graph") end
  | walk(if type == "string" then split($root) | join("/mini_racer_rs") else . end)
' "$work"/*.json >"$cdx"

spdx="$out/mini_racer_rs-$version.spdx.json"
jq --arg version "$version" '
  def id: "SPDXRef-" + gsub("[^A-Za-z0-9.]+"; "-");
  def license: [(.licenses // [])[] | .expression // .license.id // empty] | if length > 0 then join(" AND ") else "NOASSERTION" end;
  ([.metadata.component] + .components) as $all
  | ($all | map(."bom-ref" | id)) as $ids
  | if ($ids | unique | length) != ($ids | length) then error("two components map to one SPDX id") else . end
  | {
      spdxVersion: "SPDX-2.3",
      dataLicense: "CC0-1.0",
      SPDXID: "SPDXRef-DOCUMENT",
      name: "mini_racer_rs-\($version)",
      documentNamespace: "https://github.com/andymai/mini_racer_rs/releases/download/v\($version)/mini_racer_rs-\($version).spdx.json",
      creationInfo: {
        created: (.metadata.timestamp | sub("\\.[0-9]+Z$"; "Z")),
        creators: (["Tool: mini_racer_rs-scripts/sbom.sh"] + [.metadata.tools[] | "Tool: \(.name)-\(.version)"])
      },
      packages: [$all[] | {
        SPDXID: (."bom-ref" | id),
        name,
        versionInfo: .version,
        description,
        downloadLocation: ([.externalReferences[]? | select(.type == "distribution") | .url][0] // "NOASSERTION"),
        filesAnalyzed: false,
        licenseConcluded: "NOASSERTION",
        licenseDeclared: license,
        copyrightText: "NOASSERTION",
        primaryPackagePurpose: "LIBRARY",
        sourceInfo: ([.properties[]? | select(.name == "mini_racer_rs:archive_path") | .value]
          | if length > 0 then "compiled into the rusty_v8 archives from \(join(", "))" else null end),
        checksums: [(.hashes // [])[] | {algorithm: (.alg | gsub("-"; "")), checksumValue: .content}],
        externalRefs: (
          [.purl // empty | {referenceCategory: "PACKAGE-MANAGER", referenceType: "purl", referenceLocator: .}]
          + [.externalReferences[]? | select(.type == "license") | {referenceCategory: "OTHER", referenceType: "license", referenceLocator: .url}]
        )
      } | with_entries(select(.value != null and .value != []))],
      relationships: (
        [{spdxElementId: "SPDXRef-DOCUMENT", relationshipType: "DESCRIBES", relatedSpdxElement: (.metadata.component."bom-ref" | id)}]
        + [.dependencies[] | .ref as $ref | .dependsOn[] | {spdxElementId: ($ref | id), relationshipType: "DEPENDS_ON", relatedSpdxElement: (. | id)}]
      )
    }
' "$cdx" >"$spdx"
echo "$cdx"
echo "$spdx"
