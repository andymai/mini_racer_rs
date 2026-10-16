## Summary

<!-- Lead with the result. State what changes for users or contributors, then give the reason and relevant implementation detail in one or two lines. Link related issues when applicable. -->

## Checks

- [ ] I ran `scripts/gate.sh`, and it passes.
- [ ] Any behavior change that differs from mini_racer 0.22.1 has a failing-first test and an entry in `compat.toml` that records the difference and its reason.
- [ ] Upstream mini_racer suite results match `test/upstream/mini_racer.status.toml`, or this pull request regenerates the ledger.
- [ ] I made no edits under `vendor/`.
- [ ] The pull request title follows Conventional Commits.
