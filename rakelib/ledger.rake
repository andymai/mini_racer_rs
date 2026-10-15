# frozen_string_literal: true

namespace :ledger do
  desc "Merge reference and candidate results (DIR=...) into the ledger; PRUNE=1 drops tests and legs with no " \
       "result, ALLOW_REGRESSIONS=1 records a pass that now fails"
  task :merge do
    require_relative "../test/upstream/milestone_rules"

    dir = ENV.fetch("DIR") { abort "usage: rake ledger:merge DIR=<downloaded result files>" }
    path = ENV.fetch("LEDGER", "test/upstream/mini_racer.status.toml")
    rows = Dir[File.join(dir, "**", "*.jsonl")].reject { |file| file.end_with?(".events.jsonl") }
                                               .flat_map { |file| File.readlines(file, chomp: true) }
                                               .reject(&:empty?).map { |line| JSON.parse(line) }
    abort "no result files under #{dir}" if rows.empty?

    result = LedgerMerge.merge(UpstreamLedger.load(path), rows,
                               legs: UpstreamLedger.known_legs(path),
                               prune: ENV["PRUNE"] == "1",
                               allow_regressions: ENV["ALLOW_REGRESSIONS"] == "1",)
    File.write(path, UpstreamLedger.dump(result.tests, result.legs))
    warn result.warnings.join("\n") unless result.warnings.empty?
    legs = rows.map { |row| row["leg"] }.uniq.sort
    puts "#{path}: #{result.tests.size} tests; merged #{rows.size} results from #{legs.join(", ")}"
  end
end
