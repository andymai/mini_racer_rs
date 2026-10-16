# frozen_string_literal: true

namespace :ledger do
  desc "Merge reference and candidate results (DIR=...) into a suite's ledger (SUITE=mini_racer or execjs); " \
       "PRUNE=1 drops tests and legs with no result, ALLOW_REGRESSIONS=1 records a pass that now fails"
  task :merge do
    require_relative "../test/upstream/milestone_rules"
    require_relative "../test/upstream/suites"

    dir = ENV.fetch("DIR") { abort "usage: rake ledger:merge DIR=<downloaded result files>" }
    suite = ENV.fetch("SUITE", "mini_racer")
    path = UpstreamSuites.ledger(suite)
    rows = Dir[File.join(dir, "**", "*.jsonl")].reject { |file| file.end_with?(".events.jsonl") }
                                               .flat_map { |file| File.readlines(file, chomp: true) }
                                               .reject(&:empty?).map { |line| JSON.parse(line) }
                                               .select { |row| row.fetch("suite", "mini_racer") == suite }
    abort "no #{suite} results under #{dir}" if rows.empty?

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
