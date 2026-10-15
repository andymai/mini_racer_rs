# frozen_string_literal: true

require_relative "ledger"

# Turns raw results into ledger statuses. A failing test is assigned to the
# first milestone whose rule matches, in plan order; a status already in the
# ledger keeps its milestone and cause.
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

  module_function

  def milestone_for(id)
    file, klass, name = id.split("#", 3)
    spike = SPIKE_TESTS.find { |_, tests| tests.include?("#{klass}##{name}") }
    return spike.first if spike

    RULES.find { |_, rule| rule.call(file, name) }&.first || "M1"
  end

  def status_for(id, reference, candidates, existing)
    ref = reference["status"]
    return { "status" => "upstream-skip" } if ref == "skip"

    unless UpstreamLedger::PASSING.include?(ref)
      cause = existing && existing["status"] == "upstream-fail" ? existing["cause"] : nil
      return { "status" => "upstream-fail",
               "cause" => cause || "mini_racer 0.22.1: #{ref}: #{reference["message"]}".strip, }
    end

    outcomes = candidates.map { _1["status"] }.uniq
    return { "status" => "pass" } if outcomes.all? { UpstreamLedger::PASSING.include?(_1) }
    return { "status" => "flaky:flaky-#{id.split("#").last}" } if outcomes.any? { UpstreamLedger::PASSING.include?(_1) }

    kind = outcomes.any? { UpstreamLedger::CRASHING.include?(_1) } ? "crash" : "xfail"
    previous_kind, previous_target = existing&.fetch("status", nil).to_s.split(":", 2)
    return { "status" => existing["status"] } if previous_kind == "deviation"

    target = %w[xfail crash].include?(previous_kind) ? previous_target : milestone_for(id)
    { "status" => "#{kind}:#{target}" }
  end

  # The most common status across legs is the base; legs that differ become
  # overrides. Hand-set timeouts survive a merge.
  def collapse(per_leg, existing)
    return nil if per_leg.empty?

    preferred = "ruby-3.4-linux-x86_64"
    counts = per_leg.values.tally
    base = counts.max_by { |status, count| [count, per_leg[preferred] == status ? 1 : 0] }.first
    entry = base.dup
    entry["timeout_s"] = existing["timeout_s"] if existing&.key?("timeout_s")
    overrides = per_leg.reject { |_, status| status == base }
    entry["overrides"] = overrides.sort.to_h unless overrides.empty?
    entry
  end
end
