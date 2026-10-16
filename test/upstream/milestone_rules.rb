# frozen_string_literal: true

require_relative "ledger"

# Turns raw results into ledger statuses. A failing test is assigned to the
# first milestone whose rule matches, in plan order; a status already in the
# ledger keeps its milestone, cause and target.
module MilestoneRules
  RULES = [
    ["M8", ->(file, _name) { file.end_with?("test_execjs.rb") }],
    ["M6", ->(file, name) { file.end_with?("single_threaded_test.rb") || name.include?("test_forking") }],
    ["M4", ->(file, name) { file.end_with?("async_test.rb") || name.match?(/wasm|promise|microtask/) }],
    ["M2", ->(file, _name) { file.end_with?("function_test.rb", "test_object_identity_fix.rb") }],
    ["M5", ->(_file, name) { name.match?(/snapshot|warmup|flags/) }],
    ["M3", ->(_file, name) { name.match?(/memory|heap|dispose|stop|timeout|terminat|gc|idle/) }],
  ].freeze

  PREFERRED_LEG = "ruby-3.4-linux-x86_64"
  STATUS_KEYS = %w[status cause].freeze

  module_function

  def milestone_for(id)
    file, _klass, name = id.split("#", 3)
    RULES.find { |_, rule| rule.call(file, name) }&.first || "M1"
  end

  # The candidate's own results decide first: a test this gem passes is `pass`
  # even where upstream fails, so a passing candidate never meets a status that
  # strict mode would reject.
  def status_for(id, reference, candidates, previous)
    outcomes = candidates.map { |row| row["status"] }.uniq
    previous_kind, previous_target = previous&.fetch("status", nil).to_s.split(":", 2)
    return { "status" => "pass" } if outcomes.all? { |o| UpstreamLedger::PASSING.include?(o) }

    if outcomes.any? { |o| UpstreamLedger::PASSING.include?(o) }
      anchor = previous_kind == "flaky" ? previous_target : "flaky-#{id.split("#").last}"
      return { "status" => "flaky:#{anchor}" }
    end

    ref = reference["status"]
    return { "status" => "upstream-skip" } if ref == "skip" && outcomes == ["skip"]

    unless UpstreamLedger::PASSING.include?(ref) || ref == "skip"
      cause = previous_kind == "upstream-fail" ? previous["cause"] : nil
      return { "status" => "upstream-fail",
               "cause" => cause || "mini_racer 0.22.1: #{ref}: #{reference["message"]}".strip, }
    end
    return previous.slice(*STATUS_KEYS) if previous_kind == "deviation"

    kind = outcomes.any? { |o| UpstreamLedger::CRASHING.include?(o) } ? "crash" : "xfail"
    target = %w[xfail crash].include?(previous_kind) ? previous_target : milestone_for(id)
    { "status" => "#{kind}:#{target}" }
  end
end

# Folds fresh results into the ledger without losing what they do not cover:
# tests and legs absent from the results keep their statuses, and per-leg
# fields such as timeout_s survive. The ledger records every leg it has seen,
# because a leg that matches the base has no entry of its own.
module LedgerMerge
  REGRESSION_KINDS = %w[xfail crash flaky].freeze
  Result = Data.define(:tests, :legs, :warnings)

  module_function

  def merge(existing, rows, legs: [], prune: false, allow_regressions: false)
    ledger = existing.transform_values { |entry| Marshal.load(Marshal.dump(entry)) }
    warnings = []
    result_legs = rows.map { |row| row["leg"] }.uniq
    override_legs = ledger.values.flat_map { |entry| entry.fetch("overrides", {}).keys }
    known = prune ? result_legs : (legs + result_legs + override_legs).uniq
    by_test = rows.reject { |row| row["id"].start_with?("__") }.group_by { |row| row["id"] }
    by_test.each do |id, results|
      per_leg = statuses(id, results, ledger[id], allow_regressions, warnings)
      ledger[id] = apply(ledger[id], per_leg, known) unless per_leg.empty?
    end
    if prune
      (ledger.keys - by_test.keys).each do |id|
        ledger.delete(id)
        warnings << "#{id}: pruned, no result in this merge"
      end
      (legs - known).each { |leg| warnings << "#{leg}: pruned, no result in this merge" }
    end
    Result.new(tests: ledger, legs: known.sort, warnings: warnings)
  end

  def statuses(id, results, entry, allow_regressions, warnings)
    results.group_by { |row| row["leg"] }.each_with_object({}) do |(leg, leg_rows), per_leg|
      reference = leg_rows.find { |row| row["mode"] == "reference" }
      candidates = leg_rows.select { |row| row["mode"] == "candidate" }
      next unless reference && !candidates.empty?

      previous = UpstreamLedger.effective(entry, leg)
      status = MilestoneRules.status_for(id, reference, candidates, previous)
      # A regression is this gem failing where upstream still passes; a skip or
      # failure in the reference run is the platform, not a regression.
      regressed = REGRESSION_KINDS.include?(status["status"].split(":").first)
      if previous&.fetch("status", nil) == "pass" && regressed && !allow_regressions
        warnings << "#{id}: passed on #{leg}, now #{status["status"]}; kept pass (ALLOW_REGRESSIONS=1 records it)"
        status = { "status" => "pass" }
      end
      per_leg[leg] = status
    end
  end

  # The base is the status most legs share, voted over every known leg, so a
  # merge covering some legs never changes what the others inherit.
  def apply(entry, per_leg, legs)
    entry ||= {}
    keys = MilestoneRules::STATUS_KEYS
    overrides = entry.fetch("overrides", {})
    previous_base = entry.slice(*keys)
    current = legs.to_h do |leg|
      [leg, per_leg[leg] || (UpstreamLedger.effective(entry, leg).slice(*keys) unless previous_base.empty?)]
    end.compact
    base = current.values.tally.max_by do |status, count|
      [count, current[MilestoneRules::PREFERRED_LEG] == status ? 1 : 0, status == previous_base ? 1 : 0]
    end.first
    merged = entry.except("overrides", *keys).merge(base)
    new_overrides = legs.each_with_object({}) do |leg, out|
      status = current.fetch(leg, base)
      override = (overrides[leg] || {}).except(*keys).merge(status == base ? {} : status)
      out[leg] = override unless override.empty?
    end
    merged["overrides"] = new_overrides.sort.to_h unless new_overrides.empty?
    merged
  end
end
