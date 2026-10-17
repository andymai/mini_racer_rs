# frozen_string_literal: true

# Threads that each create a context, evaluate on it and dispose it, over and
# over for a while; fails on any wrong result or exception, or when resident
# memory grows more per context than the allowed bytes after a warm-up. The
# bound is not zero because rusty_v8 152.2.0 leaks a small cell for every
# isolate it creates (denoland/rusty_v8#2075; 80 bytes with glibc's malloc).
# The V8 platform mode is fixed per process, so each mode is its own run.
#   ruby -Ilib test/stress/churn.rb single|multi [threads] [seconds] [max_bytes_per_context]

require "mini_racer"

mode = ARGV.fetch(0) { abort "usage: churn.rb single|multi [threads] [seconds] [max_bytes_per_context]" }
abort "mode is single or multi, not #{mode}" unless %w[single multi].include?(mode)
threads = Integer(ARGV.fetch(1, 8))
seconds = Float(ARGV.fetch(2, 600))
max_bytes = Integer(ARGV.fetch(3, 256))
MiniRacer::Platform.set_flags!(:single_threaded) if mode == "single"
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
deadline = started + seconds
Thread.abort_on_exception = true

def rss_kb = Integer(`ps -o rss= -p #{Process.pid}`.strip)

counts = Array.new(threads, 0)
workers = Array.new(threads) do |n|
  Thread.new do
    while Process.clock_gettime(Process::CLOCK_MONOTONIC) < deadline
      context = MiniRacer::Context.new(timeout: 5_000)
      raise "wrong result" unless context.eval("#{n} + 1") == n + 1

      context.dispose if counts[n].even?
      counts[n] += 1
    end
  end
end
# The heaps of V8 and Ruby keep growing for the first stretch.
sleep(seconds / 10)
GC.start
warm_count = counts.sum
warm_rss = rss_kb
workers.each(&:join)
GC.start
churned = counts.sum - warm_count
per_context = (rss_kb - warm_rss) * 1024.0 / churned
puts format("%<total>d contexts created, evaluated and released by %<threads>d threads in %<seconds>s s; " \
            "RSS %<b>d KB -> %<a>d KB after the warm-up, %<per>.1f bytes per context",
            total: counts.sum, threads:, seconds:, b: warm_rss, a: rss_kb, per: per_context,)
abort "RSS grew #{format("%.1f", per_context)} bytes per context, more than #{max_bytes}" if per_context > max_bytes
