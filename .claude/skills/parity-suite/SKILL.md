---
name: parity-suite
description: Use when running mini_racer's own test suite against mini_racer_rs, reading or updating test/upstream/mini_racer.status.toml, regenerating the ledger after a change, debugging an upstream test that fails, hangs or crashes, or when the harness guard exits 97.
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
| `xfail:<M>` | fails until milestone `<M>` in `test/upstream/milestones.toml` |
| `crash:<M>` | hangs or crashes until `<M>`; set `timeout_s` to keep runs short |
| `deviation:<id>` | fails on purpose; `<id>` names a recorded deviation |
| `flaky:<anchor>` | result varies by seed; any outcome is accepted |

An `xfail` or `crash` test that starts passing fails strict mode on purpose: update its status to `pass` in the same PR (the ratchet).

## Regenerating

1. Add the `ledger-generate` label to the PR; `.github/workflows/ledger-generate.yml` runs the reference once and three candidate seeds on every leg.
2. `gh run watch <run-id>`, then `gh run download <run-id> -D tmp/ledger-<sha>`.
3. `bundle exec rake ledger:merge DIR=tmp/ledger-<sha>`. Tests and legs without results keep their entries; milestones, causes, deviations and `timeout_s` survive. A test that passed and now fails is kept as `pass` with a warning unless `ALLOW_REGRESSIONS=1`; `PRUNE=1` removes tests with no result.
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

Details: [reference.md](reference.md).
