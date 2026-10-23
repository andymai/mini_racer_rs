# Layer Boundaries reference

Every check prints `VIOLATION [<tag>]: <detail>` (or `[settings]`) and exits nonzero. `scripts/test-boundaries.sh` plants a violation for every tag in the first table except `settings`, which reads live GitHub settings.

| Script | Tag | Means | Fix |
|---|---|---|---|
| `scripts/check-boundaries.sh` | `members` | A workspace member is missing from `layers.toml`, or the reverse | Add the crate to both |
| | `deps` | A normal dependency is not in the crate's `normal` list, or a cfg-scoped one is not in `target_normal` for that cfg | Remove it, move the code, or add it deliberately |
| | `dev-deps` | A dev or build dependency points at a higher layer | Test through the lower layer's own API |
| | `transitive` | `magnus` or `rb-sys` is reachable from L0 or L1 | Move the code into the extension crate |
| | `engine-only` | A crate other than the engine names `v8` | Route through `mini_racer_engine` |
| | `v8-features` | A forbidden `v8` feature is on | Turn it off; the sandbox and pointer compression stay off |
| | `cdylib` | A crate's cdylib status differs from `layers.toml` | Only the extension is a cdylib |
| | `publish` | A crate is publishable | `publish.workspace = true` |
| | `forbid` | An L0 crate lacks `#![forbid(unsafe_code)]` | Restore it |
| | `claude-md` | The `CLAUDE.md` layer table differs from `layers.toml` | Update the table |
| `scripts/check-unsafe-fences.sh` | `fence` | An `unsafe_code` lint override outside a fenced `mod` line, including inner attributes and wrapped or combined lists | Move the code into a fenced module |
| | `lints` | A crate does not inherit the workspace lints | `[lints]` with `workspace = true` |
| `scripts/check-ffi-guards.sh` | `export` | A Ruby method defined with `method!` or `function!` directly | Use `export!` |
| | `extern` | An `extern "C" fn` body without `guard()` or `catch_unwind` | Wrap the body |
| `scripts/check-gc-roots.sh` | `gc-root` | `Lazy`, `Opaque`, `BoxValue`, or GC registration outside `gc_root` | Use `gc_root` types |
| `scripts/check-statics.sh` | `static` | An engine static or `thread_local!` outside `engine::process`, or any in L0 | Move it into `engine::process` |
| `scripts/check-exports.sh` | `allowlist` | Built exports differ from the committed list | Intended: `--write`; otherwise find the new `#[no_mangle]` |
| | `mangled` | A C++ or Rust mangled symbol is exported | Find what widened the export list |
| | `madvise` | The extension still imports `madvise`, or lacks `__wrap_madvise` | Restore `-Wl,--wrap=madvise` |
| | `vtable` | V8's `CustomPlatform` vtable is not the size `PlatformVtable` copies | Resize `PlatformVtable` in `crates/mini_racer_engine/src/sys.rs` after a v8 bump |
| `scripts/check-prose.sh` | `dash` | An en or em dash | Use a period, comma, colon, or parentheses |
| `scripts/check-action-pins.sh` | `pin` | An action not pinned to a full SHA with a version comment | Pin it |
| | `image` | A workflow container, `docker run` or Dockerfile stage image without a digest | Pin it to its multi-arch index digest |
| `scripts/check-versions.sh` | `version` | Version literals disagree | Make them agree |
| `scripts/check-platforms.sh` | `platform` | Target lists disagree | Make them agree |
| `scripts/check-doc-paths.sh` | `doc-path` | A path named in `CLAUDE.md`, a skill, a root Markdown doc or a GitHub template does not exist | Fix the path or the doc (for `COMPATIBILITY.md`, `compat.toml`); public docs change through codex-write and Andy's approval |
| `scripts/check-repo-settings.sh` | `settings` | A GitHub setting differs from the expected value (run locally with an admin token) | Restore the setting |

Checks `scripts/test-boundaries.sh` does not plant:

| Script | Tag | Means | Fix |
|---|---|---|---|
| `scripts/check-scheduled-watch.sh` | `watch` | A scheduled workflow is missing from, or unnamed for, the scheduled-failure watch | Add its `name:` to the list in `.github/workflows/scheduled-failures.yml` |
| `scripts/check-vendored.sh` | `vendored` | A vendored TruffleRuby file or mini_racer's license differs from `vendor/mini_racer` | Copy it again byte for byte |
| `scripts/update-notices.sh --check` | `notices` | `THIRD_PARTY_NOTICES.md` or `licenses/` is out of date, or a V8 archive links unlisted source | Run `scripts/update-notices.sh` |
| `scripts/check-glibc-floor.sh` | `glibc` | A Linux extension needs a newer glibc than the floor | Build on the older image |
| `scripts/check-macos-minos.sh` | `macos` | A macOS extension needs a newer macOS than the floor | Keep `MACOSX_DEPLOYMENT_TARGET` at the floor |
| `scripts/check-gem-sizes.sh` | `size` | A gem is over the budget in `release/size-budget.toml`, or the budget is over its ceiling | Find what grew; raise the budget only deliberately |
