# frozen_string_literal: true

# Evaluates a tiny script many times on one context and fails if resident memory
# grows more than the allowed fraction after a warm-up. V8 keeps sizing its heap
# for the first few hundred thousand evaluations (the real gem's RSS rises about
# 25% over that stretch too), so the default warm-up waits that out and what is
# left to see is a leak. The V8 platform mode is fixed per process, so each
# mode is its own run.
#   ruby -Ilib test/stress/round_trips.rb single|multi [count] [warmup] [max_growth]

require "mini_racer"

mode = ARGV.fetch(0) { abort "usage: round_trips.rb single|multi [count] [warmup] [max_growth]" }
abort "mode is single or multi, not #{mode}" unless %w[single multi].include?(mode)
count = Integer(ARGV.fetch(1, 1_000_000))
warmup = Integer(ARGV.fetch(2, 500_000))
max_growth = Float(ARGV.fetch(3, 0.05))
MiniRacer::Platform.set_flags!(:single_threaded) if mode == "single"

def rss_kb
  if File.exist?("/proc/self/status")
    File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1].to_i
  else
    Integer(`ps -o rss= -p #{Process.pid}`.strip)
  end
end

context = MiniRacer::Context.new
warmup.times { context.eval("1+1") }
GC.start
before = rss_kb
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
count.times { |i| raise "eval #{i} returned the wrong value" unless context.eval("1+1") == 2 }
elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
GC.start
after = rss_kb
growth = (after - before).fdiv(before)

puts format("%<count>d round trips in %<s>.1f s (%<us>.2f us each); RSS %<b>d KB -> %<a>d KB (%<g>+.2f%%)",
            count:, s: elapsed, us: elapsed * 1e6 / count, b: before, a: after, g: growth * 100,)
abort "RSS grew #{format("%.2f", growth * 100)}%, more than #{(max_growth * 100).round(2)}%" if growth > max_growth
