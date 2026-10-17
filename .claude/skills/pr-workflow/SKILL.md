---
name: pr-workflow
description: Use when committing, pushing, opening, reviewing, or merging a pull request in mini_racer_rs, or when a git hook fails, commitlint or the dash check rejects a message, CI Pass fails, a review bot leaves threads, or parallel work needs a worktree.
---

# PR Workflow

Every change reaches `main` as a squash-merged PR with both required checks green, `CI Pass` (GitHub Actions) and `Greptile Review` (the Greptile app, id 867647), and every review thread resolved. There is no human approval gate; CI and the review loop are the gate.

## Quick reference

| Step | Command |
|---|---|
| Branch | `git checkout main && git pull`, then `git checkout -b <type>/<kebab-description>` |
| Parallel work | `git worktree add .worktrees/<branch> -b <branch>`, then `npm ci` inside it |
| Local gate | `scripts/gate.sh`; use `--full` when a `Cargo.toml`, a script, or CI changed |
| Independent review | the `code-review` skill at high effort on the branch diff; fix every P0 and P1 |
| PR title and body | the `codex-write` skill: brief, run, fact-check, its checker prints PASS, then open |
| Push | `git push "https://x-access-token:$(gh auth token)@github.com/andymai/mini_racer_rs.git" <branch> 2>&1 \| sed -E 's/x-access-token:[^@]+@/x-access-token:***@/'` |
| Open the PR | `gh pr create --head <branch> --base main --title <title> --body-file <body-file>` (the token push sets no upstream, so pass `--head`) |
| Confirm the remote head | `gh pr view <N> --json headRefOid` |
| Watch CI | `gh pr checks <N> --watch` |
| Review threads | Greptile posts its review a few minutes after a push; handle its threads with the `check-pr-comments` skill |
| Merge | `gh pr merge <N> --auto --squash` once threads are resolved |
| After merge | when `gh pr view <N> --json state` reports `MERGED`: `git checkout main && git pull`, then remove the worktree |

## Hooks

Hooks live in `.husky/` and run under POSIX `sh`.

- `commit-msg`: commitlint with `@commitlint/config-conventional`, then `scripts/check-prose.sh --commit-msg`. Both must be installed (`npm ci`); a missing tool fails the hook.
- `pre-commit`: `cargo fmt --check`, clippy with `-D warnings`, `taplo fmt --check`, `cargo machete`, and RuboCop in parallel. It sources `scripts/fetch-v8-archive.sh` first. A missing tool fails the hook; `bin/setup` installs them.
- `pre-push`: prints a line and exits; CI is the gate.

Never skip a hook. If one fails on breakage you did not cause, stop and report it.

## Procedure

1. Branch from an up-to-date `main`.
2. Make the change with its tests; follow `.claude/skills/layer-boundaries` for anything touching crates, `unsafe`, or FFI.
3. Run `scripts/gate.sh` (or `--full`) until it exits 0 with `Gate passed (<mode>).`
4. Commit with a conventional message (`feat`, `fix`, `chore`, `docs`, `test`, `ci`, `refactor`, `perf`, `build`), no dashes.
5. Run the `code-review` skill at high effort on the branch diff and fix P0 and P1 findings with a follow-up commit.
6. Push the branch (token push above) and confirm the head with `gh pr view <N> --json headRefOid` once the PR exists.
7. Draft the PR title and body with `codex-write` (observable facts, no hard wraps), fact-check them, and open the PR with `--head`.
8. Watch CI. On failure, read the failing step with `gh run view <run> --log-failed`, fix, push.
9. Run `check-pr-comments` for review-bot threads; resolve each one.
10. Enable squash auto-merge. When the PR reports `MERGED`, return to `main`, pull, and remove the worktree.

## When CI fails

| Job or message | Fix |
|---|---|
| `VIOLATION [<tag>]` from a check script | See `.claude/skills/layer-boundaries/reference.md` for each tag |
| Check exports: allowlist differs | Intended change: `scripts/check-exports.sh --write` on Linux; for macOS commit the `exports.allow.macos` from the failing leg's `exports-macos-15-ruby-<version>` artifact |
| Gem Contents: missing or unexpected files | Adjust the globs in `mini_racer_rs.gemspec` |
| MSRV | Raise it on purpose in `Cargo.toml`, `tools/versions.toml`, and `MINIMUM_RUST` in `ext/mini_racer_rs/extconf.rb` together; `scripts/check-versions.sh` compares them |
| TOML Format | `taplo fmt` |
| RuboCop | `bundle exec rubocop -a`, then fix what remains |
| Cargo Deny or Security Audit | Read the advisory and upgrade within range. If no fix exists, record the advisory ID with its reason in both `[advisories] ignore` in `deny.toml` and the ignore list in .cargo/audit.toml (cargo audit does not read deny.toml) |

Job details: [reference.md](reference.md).
