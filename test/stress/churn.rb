# frozen_string_literal: true

# Threads that each create a context, evaluate on it and dispose it, over and
# over for a while; fails on any wrong result or exception, or when resident
# memory grows after a warm-up by more than the allowed bytes per context plus
# a fixed slack. The bound is not zero because rusty_v8 152.2.0 leaks a 64-byte
# cell for every isolate it creates (denoland/rusty_v8#2075); runs measure about
# 80 bytes per context on Linux and 175 on macOS. The slack absorbs allocator
# and GC noise, which is a fixed amount rather than one per context, so short
# runs pass too.
# The V8 platform mode is fixed per process, so each mode is its own run.
#   ruby -Ilib test/stress/churn.rb single|multi [threads] [seconds] [max_bytes_per_context]

require "mini_racer"

mode = ARGV.fetch(0) { abort "usage: churn.rb single|multi [threads] [seconds] [max_bytes_per_context]" }
abort "mode is single or multi, not #{mode}" unless %w[single multi].include?(mode)
threads = Integer(ARGV.fetch(1, 8))
seconds = Float(ARGV.fetch(2, 600))
max_bytes = Integer(ARGV.fetch(3, 256))
slack_kb = 32 * 1024
MiniRacer::Platform.set_flags!(:single_threaded) if mode == "single"
Thread.abort_on_exception = true

def rss_kb
  if File.exist?("/proc/self/status")
    File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1].to_i
  else
    Integer(`ps -o rss= -p #{Process.pid}`.strip)
  end
end

def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

def churn(threads, seconds)
  deadline = now + seconds
  counts = Array.new(threads, 0)
  Array.new(threads) do |n|
    Thread.new do
      while now < deadline
        context = MiniRacer::Context.new(timeout: 5_000)
        raise "wrong result" unless context.eval("#{n} + 1") == n + 1

        context.dispose if counts[n].even?
        counts[n] += 1
      end
    end
  end.each(&:join)
  counts.sum
end

# Measured with no context live: a context collected by GC finishes tearing
# down on its runner thread a moment later.
def settled_rss_kb
  GC.start
  sleep 1
  rss_kb
end

# The heaps of V8 and Ruby keep growing for the first stretch.
warm_count = churn(threads, seconds / 10)
warm_rss = settled_rss_kb
churned = churn(threads, seconds - (seconds / 10))
abort "no contexts were churned after the warm-up" unless churned.positive?
rss = settled_rss_kb
limit_kb = (churned * max_bytes / 1024) + slack_kb
puts format("%<total>d contexts created, evaluated and released by %<threads>d threads in %<seconds>s s; " \
            "RSS %<b>d KB -> %<a>d KB after the warm-up (%<per>.1f bytes per context), limit %<limit>d KB",
            total: warm_count + churned, threads:, seconds:, b: warm_rss, a: rss,
            per: (rss - warm_rss) * 1024.0 / churned, limit: warm_rss + limit_kb,)
abort "RSS grew #{rss - warm_rss} KB, more than #{limit_kb} KB" if rss - warm_rss > limit_kb
