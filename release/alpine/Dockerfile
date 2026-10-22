# Never built: the images the musl jobs run, which scripts/alpine-image.sh
# reads by stage name, one per Ruby in tools/versions.toml [release] rubies and
# the oldest Alpine the musl gems are smoke-tested on. Each is pinned to its
# multi-arch index digest, so a retag cannot swap what the release runs;
# Dependabot proposes new digests here, and none merges unreviewed.
FROM ruby:3.3-alpine@sha256:e9a716347e21361a3ba30bec15e15b89540b64b6ad2121e23220985f2b5bc72a AS ruby-3.3
FROM ruby:3.4-alpine@sha256:964cc9931ae515b7b4e0822f6d88e0c5e33a92033a024b242f52a0c08dd1c4cb AS ruby-3.4
FROM ruby:4.0-alpine@sha256:280685cbe09c904c386a77ea0b5097eddcf0121504340243f01dadaf51d00801 AS ruby-4.0
FROM ruby:3.4-alpine3.20@sha256:78223c2421bbd1e133fc6e126cf632c50b31c8728cbdbdae5742881c13c73350 AS floor
