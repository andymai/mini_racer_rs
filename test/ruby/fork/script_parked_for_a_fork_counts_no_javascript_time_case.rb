# frozen_string_literal: true

# A runner interrupted in a loop parks there while the fork waits for the
# other runners, and that wait is not JavaScript time. V8 does not interrupt
# JSON.parse, so a long parse elsewhere holds the fork for its 500 ms bound;
# the input grows until one parse outlasts it.
require_relative "prelude"

def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

def dispatched(context, count)
  deadline = now + 10
  sleep 0.001 until MiniRacerRs.counters(context)[:dispatches] >= count || now > deadline
end

looper = MiniRacer::Context.new
parser = MiniRacer::Context.new

held = [4_000_000, 8_000_000, 16_000_000, 32_000_000].any? do |pairs|
  parser.eval("globalThis.big = JSON.stringify(Array.from({length: #{pairs}}, (_, i) => [i, i]))")
  before = MiniRacerRs.counters(looper)
  parses = MiniRacerRs.counters(parser)[:dispatches]
  started = now
  looping = Thread.new { looper.eval("{ const end = Date.now() + 1500; while (Date.now() < end) {} }") }
  dispatched(looper, before[:dispatches] + 1)
  parsing = Thread.new { parser.eval("JSON.parse(big).length") }
  dispatched(parser, parses + 1)
  sleep 0.02
  forked = now
  child { nil }
  waited = now - forked
  looping.join
  wall = now - started
  raise "parser" unless parsing.value == pairs
  next false if waited < 0.45

  ran = (MiniRacerRs.counters(looper)[:js_ns] - before[:js_ns]) / 1e9
  # The fork interrupts the loop 50 ms into its wait and the loop parks until
  # the fork is done; a loaded runner parks later, so at least half the wait
  # must be left out. Counted whole, it would reach the wall time.
  raise "js_ns #{ran} s of a #{wall} s loop that waited #{waited} s for the fork" if ran > wall - (waited / 2)

  true
end
raise "no parse outlasted the fork's wait" unless held
