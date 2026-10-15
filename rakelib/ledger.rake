# frozen_string_literal: true

namespace :ledger do
  desc "Merge reference and candidate results (DIR=...) into test/upstream/mini_racer.status.toml"
  task :merge do
    require_relative "../test/upstream/ledger"
    require_relative "../test/upstream/milestone_rules"

    dir = ENV.fetch("DIR") { abort "usage: rake ledger:merge DIR=<downloaded result files>" }
    path = ENV.fetch("LEDGER", "test/upstream/mini_racer.status.toml")
    rows = Dir[File.join(dir, "**", "*.jsonl")].reject { _1.end_with?(".events.jsonl") }
                                               .flat_map { File.readlines(_1, chomp: true) }
                                               .reject(&:empty?).map { JSON.parse(_1) }
    abort "no result files under #{dir}" if rows.empty?

    existing = UpstreamLedger.load(path)
    by_test = rows.group_by { _1["id"] }
    merged = by_test.to_h do |id, results|
      per_leg = results.group_by { _1["leg"] }.filter_map do |leg, leg_rows|
        reference = leg_rows.find { _1["mode"] == "reference" }
        candidates = leg_rows.select { _1["mode"] == "candidate" }
        next unless reference && !candidates.empty?

        [leg, MilestoneRules.status_for(id, reference, candidates, existing[id])]
      end.to_h
      [id, MilestoneRules.collapse(per_leg, existing[id])]
    end
    File.write(path, UpstreamLedger.dump(merged.compact))
    legs = rows.map { |row| row["leg"] }.uniq.size
    puts "#{path}: #{merged.compact.size} tests from #{rows.size} results across #{legs} legs"
  end
end
