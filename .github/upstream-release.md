RubyGems.org lists mini_racer `{{ version }}` as released, while mini_racer_rs targets mini_racer `{{ target }}`.

- [ ] Read the `{{ version }}` changelog and review the release diff for `test/` and `lib/`.
- [ ] Run the `{{ version }}` test suite against mini_racer_rs and record the results, including each failure and platform.
- [ ] Reflect the `{{ version }}` public surface with the reference bundle at that release, run `rake compat:verify`, and list every new or changed public surface item.
- [ ] Decide whether to move `vendor/mini_racer` and update `tools/versions.toml` to target `{{ version }}`. Record any deliberate difference with its reason and a test.
- [ ] Close this issue with the decision, the selected target, and links to any resulting pull requests or follow-up issues.
