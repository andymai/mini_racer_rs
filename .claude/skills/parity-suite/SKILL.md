---
name: parity-suite
description: Use when running mini_racer's own test suite against mini_racer_rs, reading or updating test/upstream/mini_racer.status.toml, regenerating the ledger after a change, debugging an upstream test that fails, hangs or crashes, when the harness guard exits 97, or when changing the public surface, compat.toml, COMPATIBILITY.md or the generated stubs.
---

# Parity Suite

Parity means mini_racer 0.22.1's own tests pass unmodified. `vendor/mini_racer` is that release as a submodule and is never edited. `test/upstream/run.rb` runs its suite against this gem (or, with `--reference`, against the real mini_racer from `tools/reference`) and compares every result with the ledger.

## Quick reference

| Task | Command |
|---|---|
| Run the suite, strict | `bundle exec ruby test/upstream/run.rb --strict` |
| One test | `bundle exec ruby test/upstream/run.rb -n test_it_can_stop` |
| One file | `bundle exec ruby test/upstream/run.rb --files test/function_test.rb` |
| Real mini_racer (reference) | `cd tools/reference && BUNDLE_PATH=vendor/bundle bundle install` (a relative `BUNDLE_PATH` resolves against the Gemfile's directory), then add `--reference` |
| Standalone scripts | `bundle exec ruby test/upstream/run.rb --scripts` |
| Results and logs | `tmp/upstream/` (one JSON line per test, plus the suite log) |
| Regenerate the ledger on a PR | add the `ledger-generate` label, then see "Regenerating" |

## Ledger statuses

Each test in `test/upstream/mini_racer.status.toml` has a `status`; `overrides."<engine>-<minor>-<os>-<arch>"` changes it for one platform.

| Status | Promise |
|---|---|
| `pass` | passes; anything else is a regression |
| `upstream-skip` | the test skips itself here |
| `upstream-fail` | the real mini_racer fails here too (`cause` required) |
| `xfail:<M>` | fails, errors or skips (never crashes or hangs) until milestone `<M>` in `test/upstream/milestones.toml` |
| `crash:<M>` | hangs, crashes or fails until `<M>`; set `timeout_s` to keep runs short |
| `deviation:<id>` | fails on purpose; `<id>` names a recorded deviation |
| `flaky:<anchor>` | result varies by seed; any outcome is accepted |

An `xfail` or `crash` test that starts passing fails strict mode on purpose: update its status to `pass` in the same PR (the ratchet).

## Regenerating

1. Add the `ledger-generate` label to the PR; `.github/workflows/ledger-generate.yml` runs the reference once and three candidate seeds on every leg.
2. `gh run watch <run-id>`, then `gh run download <run-id> -D tmp/ledger-<sha>`.
3. `bundle exec rake ledger:merge DIR=tmp/ledger-<sha>`. The ledger's `legs` list records every leg merged so far; a test's base status is the one most of those legs share, so a merge from fewer legs never changes what the others inherit. Tests and legs without results keep their statuses; milestones, causes, deviations and `timeout_s` survive. A test that passed and now fails is kept as `pass` with a warning unless `ALLOW_REGRESSIONS=1`; `PRUNE=1` removes tests and legs with no result.
4. Review the diff, commit it, and remove the label.

Locally, write runs into a fresh directory with `--out <dir>/<name>.jsonl` (one reference, several candidate seeds) and merge that directory; old runs in it would count as extra seeds.

## When something goes wrong

| Symptom | Meaning | Next step |
|---|---|---|
| exit 97, "the suite must exercise mini_racer_rs" | A file from upstream's `lib/` loaded | Find what required it; never weaken `test/upstream/preload.rb` |
| `timeout` result | The watchdog killed a test that overran | Read `tmp/upstream/<test>.timeout.txt` (Ruby threads, then native stacks) |
| `crash` result | The suite process died, or called exit, inside that test | The suite log in `tmp/upstream/` has the last output |
| `suite failure` | The process crashed or hung outside any test (loading, between tests, at exit) | Strict mode always fails on it; read the log and `tmp/upstream/suite-attempt-*.timeout.txt` |
| "not in the ledger" | A new upstream test or a renamed one | Regenerate |
| "in the ledger but did not run" | A ledger entry for a test that no longer exists | Regenerate |

## Compatibility ledger

`compat/surface.json` lists every public id of mini_racer 0.22.1 (constants, classes with their superclass, instance and singleton methods, keyword arguments, required standard libraries), reflected from the real gem on CRuby and TruffleRuby; an id found on one engine only ends in `@ruby` or `@truffleruby`. `compat.toml` gives each id a status for CRuby: `done`, `todo:<milestone>`, or `deviation` with a `reason` (an `approved` date is the owner's to add). On TruffleRuby mini_racer_rs runs upstream's backend, so its surface must match exactly.

| Task | Command |
|---|---|
| Check the ledger against this gem | `bundle exec rake compat:check` |
| Regenerate COMPATIBILITY.md, the README table and `lib/mini_racer_rs/stubs.rb` | `bundle exec rake compat:render` |
| Implemented a planned method | set its id to `done`, then `rake compat:render` (its stub disappears) |
| Re-reflect the reference | `REFERENCE=1 bundle exec rake compat:reflect` on CRuby and on TruffleRuby, then `bundle exec rake compat:surface` |

`compat:check` failures: `MISSING` (an id has no entry), `STALE` (an entry for an id that left the surface), `MALFORMED`, `UNKNOWN_MILESTONE`, `CLAIMED_DONE_BUT_ABSENT` (done, but the method is absent or still a stub), `TODO_BUT_IMPLEMENTED` (mark it done), `EXTRA_SURFACE` (this gem exposes something mini_racer does not). Keyword arguments of native methods cannot be reflected on CRuby; the upstream suite covers them.

Details: [reference.md](reference.md).
