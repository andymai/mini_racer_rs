---
name: parity-suite
description: Use when running mini_racer's or ExecJS's test suite against mini_racer_rs, reading or updating test/upstream/mini_racer.status.toml or test/upstream/execjs.status.toml, regenerating the ledger after a change, debugging an upstream test that fails, hangs or crashes, when the harness guard exits 97, when running or recording the differential run against the real gem (test/diff/run.rb, test/diff/candidate.status.toml, test/diff/divergences.toml, the consumer workloads), or when changing the public surface, compat.toml or COMPATIBILITY.md.
---

# Parity Suite

Parity means mini_racer 0.22.2's own tests pass unmodified. `vendor/mini_racer` is that release as a submodule and is never edited. `test/upstream/run.rb` runs its suite against this gem (or, with `--reference`, against the real mini_racer from `tools/reference`) and compares every result with the ledger. `--suite execjs` runs ExecJS 2.10.2's suite (`vendor/execjs`) the same way with `EXECJS_RUNTIME=MiniRacer`, against `test/upstream/execjs.status.toml`. `test/upstream/run.rb` sets `BUNDLE_GEMFILE`, `RUBYOPT`, `MINI_RACER_RS_UPSTREAM_REPORT`, `MINI_RACER_RS_UPSTREAM_ROOT` and, for ExecJS, `EXECJS_RUNTIME` (reference.md), so run the suites through it, not with `rake test` inside the submodule.

## Quick reference

| Task | Command |
|---|---|
| First run in a checkout or worktree | `git submodule update --init`, which the upstream suites and `rake test` (the ExecJS autodetect test) need |
| Run the suite, strict | `bundle exec ruby test/upstream/run.rb --strict` |
| ExecJS's suite, strict | `bundle exec ruby test/upstream/run.rb --suite execjs --strict` |
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

1. Add the `ledger-generate` label to the PR; `.github/workflows/ledger-generate.yml` runs the reference once and three candidate seeds of both suites on every leg.
2. `gh run watch <run-id>`, then `gh run download <run-id> -D tmp/ledger-<sha>`.
3. `bundle exec rake ledger:merge DIR=tmp/ledger-<sha>`, then again with `SUITE=execjs`. The ledger's `legs` list records every leg merged so far; a test's base status is the one most of those legs share, so a merge from fewer legs never changes what the others inherit. Tests and legs without results keep their statuses; milestones, causes, deviations and `timeout_s` survive. A test that passed and now fails is kept as `pass` with a warning unless `ALLOW_REGRESSIONS=1`; `PRUNE=1` removes tests and legs with no result.
4. Review the diff, commit it, and remove the label.

Locally, write runs into a fresh directory with `--out <dir>/<name>.jsonl` (one reference, several candidate seeds) and merge that directory; old runs in it would count as extra seeds.

## When something goes wrong

| Symptom | Meaning | Next step |
|---|---|---|
| exit 97, "the suite must exercise mini_racer_rs" | A file from upstream's `lib/` loaded | Find what required it; never weaken `test/upstream/preload.rb` |
| `timeout` result | The watchdog killed a test that overran | Read `tmp/upstream/<test>.timeout.txt` (Ruby threads, then native stacks) |
| `crash` result | The suite process died, or called exit, inside that test | The suite log in `tmp/upstream/` has the last output |
| `suite failure` | The process crashed or hung outside any test (loading, between tests, at exit) | Strict mode always fails on it; read the log and `tmp/upstream/<suite>-suite-attempt-<n>.timeout.txt` |
| "not in the ledger" | A new upstream test or a renamed one | Regenerate |
| "in the ledger but did not run" | A ledger entry for a test that no longer exists | Regenerate |

## Compatibility ledger

`compat/surface.json` lists every public id of mini_racer 0.22.2 (constants, classes with their superclass, instance and singleton methods, keyword arguments, required standard libraries), reflected from the real gem on CRuby and TruffleRuby; an id found on one engine only ends in `@ruby` or `@truffleruby`. Keywords that upstream's C parses by hand are found by calling with each one (`KEYWORD_PROBES` in `tools/compat/reflect.rb`). `compat.toml` gives each id a status for CRuby: `done`, `todo:<milestone>`, or `deviation` with a `reason` (an `approved` date is the owner's to add). A surface deviation means the id is absent on purpose; a behavior difference gets its own `behavior:<kebab>` id, optionally engine-scoped. On TruffleRuby mini_racer_rs runs upstream's backend, so its surface must match exactly.

What this gem adds under `MiniRacerRs` is reflected apart, as the candidate report's `extra_ids`, and recorded in `compat.toml`'s `[extra]` table by the same id grammar with `status = "done"`, or `"internal"` for what callers never use (`MiniRacerRs::Pins`, `Spill`, `ForkHook`); `COMPATIBILITY.md` lists them under Additions. On TruffleRuby the native extension never loads, so `MiniRacerRs` holds only `VERSION`, and an id the extension defines ends in `@ruby`.

| Task | Command |
|---|---|
| Check the ledger against this gem | `bundle exec rake compat:check` |
| Also re-reflect the real gem | `bundle exec rake compat:verify` (needs the `tools/reference` bundle) |
| Regenerate COMPATIBILITY.md and the README table | `bundle exec rake compat:render` |
| Implemented a planned method | set its id to `done`, then `rake compat:render` |
| Added a `MiniRacerRs` method, constant or class | add `[extra."smethod:MiniRacerRs.<name>@ruby"]` with `status = "done"`, then `rake compat:render` |
| Rebuild the surface | `REFERENCE=1 bundle exec rake compat:reflect` on CRuby and on TruffleRuby, then `bundle exec rake compat:surface` |

`compat:check` failures: `MISSING` (an id has no entry), `STALE` (an entry for an id that left the surface), `MALFORMED`, `UNKNOWN_MILESTONE`, `CLAIMED_DONE_BUT_ABSENT`, `TODO_BUT_IMPLEMENTED` (mark it done), `DEVIATION_BUT_PRESENT`, `EXTRA_SURFACE` (this gem exposes something mini_racer does not), `UNKNOWN_DEVIATION` (an upstream ledger cites a deviation compat.toml lacks), `SURFACE_DRIFT` (`compat:verify` only: the real gem and compat/surface.json disagree), `MISSING_EXTRA` (a `MiniRacerRs` id with no `[extra]` entry), `STALE_EXTRA` (an `[extra]` entry this gem lacks on the engine checked). A planned method that exists before what it calls works (`write_heap_snapshot`) lists that in `depends_on`. `rake` needs the lint group installed; without it (the TruffleRuby CI job), run `tools/compat/reflect.rb` and `tools/compat/check.rb` directly.

## Differential run

`test/diff/run.rb` sends the same cases to the real mini_racer 0.22.2 and to this gem, each in its own worker process, and compares the outcomes in a canonical form that keeps type, encoding, float bits and which containers are the same object. Cases: seeded JS values (`--js`, evaluated), seeded Ruby values (`--ruby`, passed through `Context#call` and returned), and `test/diff/corpus/*.json`.

| Task | Command |
|---|---|
| Check this gem against the ledger | `bundle exec ruby test/diff/run.rb --strict` |
| Also prove the harness is deterministic | add `--twin`: a second copy of the real gem must agree with the first on every case |
| Record outcomes after a change | `bundle exec ruby test/diff/run.rb --write-ledger` (a new divergence becomes `xfail:M1`; existing milestones stay) |
| Minimize new divergences | `bundle exec ruby test/diff/run.rb --shrink` (writes `test/diff/corpus/min-<id>.json`) |
| A fresh seed, no ledger | `bundle exec ruby test/diff/run.rb --explore --shrink --seed <n>`: its cases share no object, so it accepts none of the `behavior:object-ids@ruby` divergences, and any divergence the other accepted ones miss, or a crash, fails the run; the nightly workflow runs fresh seeds for 30 minutes |
| A seed's shared values, no ledger | `bundle exec ruby test/diff/run.rb --explore --shared --shrink --seed <n> --js 1500 --ruby 1500`: its cases share objects as the ledger's do, so the `behavior:object-ids@ruby` divergences stay accepted (the `lost-reference` structure in `test/diff/shapes.rb`), and any other divergence, or a crash, fails the run |
| Real libraries instead of generated values | `npm ci && npm run build` in `test/consumers/js`, then `bundle exec ruby test/diff/run.rb --set consumers --twin --strict` (ledger `test/consumers/candidate.status.toml`) |
| A gem that depends on mini_racer resolves to this one | `BUNDLE_GEMFILE=$PWD/test/consumers/transitive/Gemfile bundle exec ruby test/consumers/transitive/check.rb` (rtlcss's own status is in `test/consumers/transitive/status.toml`) |
| Numbers against the real gem | `bundle exec ruby bench/run.rb`, then `bundle exec ruby bench/floors.rb tmp/bench/<result>.json` for the floors the weekly run keeps (needs `npm ci && npm run build` in `test/consumers/js` for the workloads; `--quick` for a smoke run) |

Each worker first reports the gem it loaded, so a broken reference bundle aborts the run instead of matching itself. `test/diff/candidate.status.toml` holds `same`, `accepted:<divergence>`, `xfail:<milestone>` (diverges) or `crash:<milestone>` (the candidate crashes or hangs) per case. An accepted divergence lives in `test/diff/divergences.toml`, matches one step, and must name a `compat.toml` deviation. Results and worker logs land in `tmp/diff/`.

Details: [reference.md](reference.md).
