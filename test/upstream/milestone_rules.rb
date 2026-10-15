# frozen_string_literal: true

require_relative "ledger"

# Turns raw results into ledger statuses. A failing test is assigned to the
# first milestone whose rule matches, in plan order; a status already in the
# ledger keeps its milestone, cause and target.
module MilestoneRules
  SPIKE_TESTS = {
    "S2" => %w[MiniRacerTest#test_it_can_stop MiniRacerTest#test_ruby_timeout_does_not_leave_stale_termination
               MiniRacerTest#test_thread_kill_waiting_thread_does_not_terminate_active_eval
               MiniRacerTest#test_ruby_signal_handler_does_not_terminate_running_javascript],
    "S3" => %w[MiniRacerFunctionTest#test_fun],
    "S3b" => %w[MiniRacerTest#test_can_attach_functions MiniRacerTest#test_return_unknown
                MiniRacerTest#test_return_date MiniRacerTest#test_datetime_missing],
    "S6" => %w[MiniRacerTest#test_forking],
  }.freeze

  RULES = [
    ["S6", ->(file, name) { file.end_with?("single_threaded_test.rb") && name.start_with?("test_fork_") }],
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
    file, klass, name = id.split("#", 3)
    spike = SPIKE_TESTS.find { |_, tests| tests.include?("#{klass}##{name}") }
    return spike.first if spike

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
# tests and legs absent from the results keep their entries, and per-leg
# fields such as timeout_s survive.
module LedgerMerge
  REGRESSION_KINDS = %w[xfail crash flaky].freeze

  module_function

  def merge(existing, rows, prune: false, allow_regressions: false)
    ledger = existing.transform_values { |entry| Marshal.load(Marshal.dump(entry)) }
    warnings = []
    by_test = rows.reject { |row| row["id"].start_with?("__") }.group_by { |row| row["id"] }
    by_test.each do |id, results|
      per_leg = statuses(id, results, ledger[id], allow_regressions, warnings)
      ledger[id] = apply(ledger[id], per_leg) unless per_leg.empty?
    end
    if prune
      (ledger.keys - by_test.keys).each do |id|
        ledger.delete(id)
        warnings << "#{id}: pruned, no result in this merge"
      end
    end
    [ledger, warnings]
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

  def apply(entry, per_leg)
    entry ||= {}
    overrides = entry.fetch("overrides", {})
    base = if per_leg.key?(MilestoneRules::PREFERRED_LEG) || entry["status"].nil?
             per_leg.values.tally.max_by do |status, count|
               [count, per_leg[MilestoneRules::PREFERRED_LEG] == status ? 1 : 0]
             end.first
           else
             entry.slice(*MilestoneRules::STATUS_KEYS)
           end
    merged = entry.except("overrides", *MilestoneRules::STATUS_KEYS).merge(base)
    legs = (overrides.keys + per_leg.keys).uniq
    new_overrides = legs.each_with_object({}) do |leg, out|
      extra = (overrides[leg] || {}).except(*MilestoneRules::STATUS_KEYS)
      status = per_leg[leg] || overrides[leg]&.slice(*MilestoneRules::STATUS_KEYS) || {}
      status = {} if status == base
      override = extra.merge(status)
      out[leg] = override unless override.empty?
    end
    merged["overrides"] = new_overrides.sort.to_h unless new_overrides.empty?
    merged
  end
end
