#!/usr/bin/env bash
# Proves each rule in the check scripts: plants one violation per rule in a
# scratch copy of the tree and requires the matching check to fail with its tag.
set -euo pipefail
cd "$(dirname "$0")/.."

exec python3 - "$PWD" <<'PY'
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

root = pathlib.Path(sys.argv[1])
listing = subprocess.run(
    ["git", "ls-files", "-co", "--exclude-standard", "-z"],
    cwd=root, capture_output=True, check=True,
).stdout.split(b"\0")

work = pathlib.Path(tempfile.mkdtemp(prefix="test-boundaries-"))
pristine = work / "pristine"
for raw in filter(None, listing):
    src = root / os.fsdecode(raw)
    if src.is_file():
        dst = pristine / os.fsdecode(raw)
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)


def replace(path, old, new):
    return ("replace", path, old, new)


def append(path, text):
    return ("append", path, text)


def write(path, text):
    return ("write", path, text)


def cc(path, source):
    return ("cc", path, source)


CRATE = '[package]\nedition = "2024"\nname = "mini_racer_extra"\npublish = false\nversion = "0.0.0"\n\n[lints]\nworkspace = true\n'

CASES = [
    ("member missing from layers.toml", "check-boundaries.sh", "members", [
        replace("Cargo.toml", '  "crates/mini_racer_engine",\n', '  "crates/mini_racer_engine",\n  "crates/mini_racer_extra",\n'),
        write("crates/mini_racer_extra/Cargo.toml", CRATE),
        write("crates/mini_racer_extra/src/lib.rs", "#![forbid(unsafe_code)]\n"),
    ]),
    ("undeclared normal dependency", "check-boundaries.sh", "deps", [
        replace("crates/mini_racer_wire/Cargo.toml", "[dependencies]\n", "[dependencies]\nlibc.workspace = true\n"),
    ]),
    ("loom outside cfg(loom)", "check-boundaries.sh", "deps", [
        replace("crates/mini_racer_sync/Cargo.toml", "\n\n[target.'cfg(loom)'.dependencies]\n", "\n"),
    ]),
    ("cfg-scoped dependency not listed for its cfg", "check-boundaries.sh", "deps", [
        replace("crates/mini_racer_sync/Cargo.toml", "loom.workspace = true\n", "libc.workspace = true\nloom.workspace = true\n"),
    ]),
    ("dev-dependency on a higher layer", "check-boundaries.sh", "dev-deps", [
        replace("crates/mini_racer_wire/Cargo.toml", "[dev-dependencies]\n", "[dev-dependencies]\nmini_racer_engine.workspace = true\n"),
    ]),
    ("Ruby crate reached from L0", "check-boundaries.sh", "transitive", [
        replace("crates/mini_racer_sync/Cargo.toml", "[dependencies]\n", "[dependencies]\nmagnus.workspace = true\n"),
        replace("layers.toml", 'normal = ["thiserror"]\npath = "crates/mini_racer_sync"', 'normal = ["magnus", "thiserror"]\npath = "crates/mini_racer_sync"'),
    ]),
    ("v8 named outside the engine", "check-boundaries.sh", "engine-only", [
        replace("ext/mini_racer_rs/Cargo.toml", "rb-sys.workspace = true\n", "rb-sys.workspace = true\nv8.workspace = true\n"),
        replace("layers.toml", '"rb-sys", "thiserror"]', '"rb-sys", "thiserror", "v8"]'),
    ]),
    ("forbidden v8 feature", "check-boundaries.sh", "v8-features", [
        replace("crates/mini_racer_engine/Cargo.toml", "v8.workspace = true\n", 'v8 = { workspace = true, features = ["v8_enable_sandbox"] }\n'),
    ]),
    ("second cdylib", "check-boundaries.sh", "cdylib", [
        append("crates/mini_racer_wire/Cargo.toml", '\n[lib]\ncrate-type = ["cdylib", "rlib"]\n'),
    ]),
    ("publishable crate", "check-boundaries.sh", "publish", [
        replace("crates/mini_racer_sync/Cargo.toml", "publish.workspace = true", "publish = true"),
    ]),
    ("L0 crate without forbid(unsafe_code)", "check-boundaries.sh", "forbid", [
        replace("crates/mini_racer_wire/src/lib.rs", "#![forbid(unsafe_code)]\n", ""),
    ]),
    ("CLAUDE.md layer table drift", "check-boundaries.sh", "claude-md", [
        write("CLAUDE.md", "| `mini_racer_wire` | L1 |\n"),
    ]),
    ("unsafe_code allowed on an unfenced module", "check-unsafe-fences.sh", "fence", [
        replace("ext/mini_racer_rs/src/lib.rs", "mod context;\n", '#[allow(unsafe_code, reason = "planted")]\nmod context;\n'),
    ]),
    ("rustfmt-wrapped allow on an unfenced module", "check-unsafe-fences.sh", "fence", [
        replace("ext/mini_racer_rs/src/lib.rs", "mod context;\n", '#[allow(\n    unsafe_code,\n    reason = "planted"\n)]\nmod context;\n'),
    ]),
    ("unsafe_code inside a combined lint list", "check-unsafe-fences.sh", "fence", [
        replace("ext/mini_racer_rs/src/lib.rs", "mod context;\n", '#[allow(dead_code, unsafe_code, reason = "planted")]\nmod context;\n'),
    ]),
    ("inner allow inside a module file", "check-unsafe-fences.sh", "fence", [
        replace("ext/mini_racer_rs/src/context.rs", "use std::sync::OnceLock;\n", '#![allow(unsafe_code, reason = "planted")]\n\nuse std::sync::OnceLock;\n'),
    ]),
    ("crate opts out of workspace lints", "check-unsafe-fences.sh", "lints", [
        replace("crates/mini_racer_sync/Cargo.toml", "[lints]\nworkspace = true\n", ""),
    ]),
    ("Ruby method defined without export!", "check-ffi-guards.sh", "export", [
        replace("ext/mini_racer_rs/src/context.rs", "export!(method Context::stop, 0)", "magnus::method!(Context::stop, 0)"),
    ]),
    ("unguarded extern fn", "check-ffi-guards.sh", "extern", [
        append("ext/mini_racer_rs/src/guard.rs", '\npub extern "C" fn planted() -> i32 {\n    1\n}\n'),
    ]),
    ("GC root outside gc_root", "check-gc-roots.sh", "gc-root", [
        append("ext/mini_racer_rs/src/errors.rs", "\nstatic PLANTED: magnus::value::Lazy<magnus::RString> = magnus::value::Lazy::new(|r| r.str_new(\"\"));\n"),
    ]),
    ("engine static outside process", "check-statics.sh", "static", [
        append("crates/mini_racer_engine/src/context.rs", "\nstatic PLANTED: u8 = 0;\n"),
    ]),
    ("extension still imports madvise", "check-exports.sh fake.so", "madvise", [
        cc("fake.so", "int madvise(void *, unsigned long, int);\nint Init_mini_racer_rs(void) { return madvise(0, 0, 0); }\n"),
    ]),
    ("export missing from the allowlist", "check-exports.sh fake.so", "allowlist", [
        cc("fake.so", "void Init_mini_racer_rs(void) {}\nvoid planted_export(void) {}\n"),
    ]),
    ("mangled symbol exported", "check-exports.sh fake.so", "mangled", [
        cc("fake.so", "void Init_mini_racer_rs(void) {}\nvoid planted(void) __asm__(\"_ZN7planted4testEv\");\nvoid planted(void) {}\n"),
    ]),
    ("stale path in CLAUDE.md", "check-doc-paths.sh", "doc-path", [
        append("CLAUDE.md", "\nSee `crates/mini_racer_engine/src/gone.rs`.\n"),
    ]),
    ("em dash in prose", "check-prose.sh", "dash", [
        append("README.md", "\nplanted \u2014 dash\n"),
    ]),
    ("action pinned by tag", "check-action-pins.sh", "pin", [
        write(".github/workflows/planted.yml", "on: push\njobs:\n  x:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n"),
    ]),
    ("gem and Cargo versions disagree", "check-versions.sh", "version", [
        replace("lib/mini_racer_rs/version.rb", 'VERSION = "0.0.0"', 'VERSION = "9.9.9"'),
    ]),
    ("platform lists disagree", "check-platforms.sh", "platform", [
        replace("deny.toml", '  "x86_64-apple-darwin",\n', ""),
    ]),
    ("transitive lockfile lacks a platform", "check-platforms.sh", "platform", [
        replace("test/consumers/transitive/Gemfile.lock", "  arm64-darwin\n", ""),
    ]),
]

env = dict(os.environ, CARGO_NET_OFFLINE="true")
failures = 0
for name, script, tag, ops in CASES:
    case = work / name.replace(" ", "-")
    shutil.copytree(pristine, case, symlinks=True)
    for op in ops:
        path = case / op[1]
        if op[0] == "replace":
            text = path.read_text()
            if text.count(op[2]) != 1:
                print(f"FAIL {name}: setup could not find {op[2]!r} once in {op[1]}")
                failures += 1
                break
            path.write_text(text.replace(op[2], op[3]))
        elif op[0] == "append":
            with path.open("a") as f:
                f.write(op[2])
        elif op[0] == "cc":
            subprocess.run(["cc", "-shared", "-fPIC", "-x", "c", "-", "-o", str(path)], input=op[2], text=True, check=True)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(op[2])
    else:
        command, *args = script.split()
        result = subprocess.run([str(case / "scripts" / command), *args], cwd=case, env=env, capture_output=True, text=True)
        output = result.stdout + result.stderr
        if result.returncode != 0 and f"[{tag}]" in output:
            print(f"ok   {name} ({script} [{tag}])")
        else:
            print(f"FAIL {name}: {script} exited {result.returncode} without [{tag}]")
            print("     " + output.strip().replace("\n", "\n     "))
            failures += 1

shutil.rmtree(work)
print(f"{len(CASES) - failures}/{len(CASES)} planted violations caught.")
sys.exit(1 if failures else 0)
PY
