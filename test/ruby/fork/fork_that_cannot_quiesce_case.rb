# frozen_string_literal: true

# V8 does not interrupt JSON.parse, so a fork during a long one waits out its
# bound and the child, unsure what V8 was in the middle of, leaves every
# inherited context alone; new ones still work. mini_racer 0.22.2 forks at once
# and its child uses the idle context. A parse that ends before the bound lets
# the fork through, so the input grows until one outlasts it.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)
idle = MiniRacer::Context.new
idle.eval("var answer = 42")
busy = MiniRacer::Context.new

CHILD_EXITS = [0, 4].freeze

blocked = [4_000_000, 8_000_000, 16_000_000, 32_000_000].any? do |pairs|
  busy.eval("globalThis.big = JSON.stringify(Array.from({length: #{pairs}}, (_, i) => [i, i]))")
  parser = Thread.new { busy.eval("JSON.parse(big).length") }
  sleep 0.02
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  pid = fork do
    refused = raised(MiniRacer::RuntimeError) { idle.eval("answer") }&.start_with?("single-threaded runner:")
    exit!(3) unless MiniRacer::Context.new.eval("1") == 1
    exit!(refused ? 0 : 4)
  end
  _, status = Process.wait2(pid)
  elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  raise "parser" unless parser.value == pairs
  raise "child exited #{status.exitstatus}" unless CHILD_EXITS.include?(status.exitstatus)

  refused = status.exitstatus.zero?
  raise "refused after only #{elapsed} s" if refused && elapsed < 0.45

  refused
end
raise "no parse outlasted the fork's wait" unless blocked
