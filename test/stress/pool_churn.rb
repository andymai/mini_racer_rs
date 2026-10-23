# frozen_string_literal: true

# Threads that check contexts out of one pool, half as large, and evaluate on
# them over and over for a while, with a use limit low enough that the pool's
# own threads replace a context every few checkouts; some checkouts raise,
# break or dispose their context on the way out. Fails on any wrong result or
# unexpected exception, or when resident memory grows after a warm-up by more
# than the allowed bytes per context built plus a fixed slack, as in churn.rb,
# whose bound this shares for the same reason (denoland/rusty_v8#2075).
# The V8 platform mode is fixed per process, so each mode is its own run.
#   ruby -Ilib test/stress/pool_churn.rb single|multi [threads] [seconds] [max_bytes_per_context]

require "mini_racer"

mode = ARGV.fetch(0) { abort "usage: pool_churn.rb single|multi [threads] [seconds] [max_bytes_per_context]" }
abort "mode is single or multi, not #{mode}" unless %w[single multi].include?(mode)
threads = Integer(ARGV.fetch(1, 8))
seconds = Float(ARGV.fetch(2, 600))
max_bytes = Integer(ARGV.fetch(3, 256))
slack_kb = 32 * 1024
max_uses = 20
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

class Expected < StandardError; end

OTHER_ENDS = %i[broke disposed].freeze

built = 0
pool = MiniRacerRs::Pool.new(size: [threads / 2, 1].max, max_uses:, timeout: 5_000) do |context|
  built += 1
  context.eval("var uses = 0")
end

churn = lambda do |duration|
  deadline = now + duration
  counts = Array.new(threads, 0)
  Array.new(threads) do |n|
    Thread.new do
      while now < deadline
        count = counts[n] += 1
        begin
          result = pool.with do |context|
            uses = context.eval("++uses")
            raise "a context ran #{uses} checkouts, past #{max_uses}" if uses > max_uses
            raise Expected if (count % 53).zero?
            break :broke if (count % 59).zero?

            if (count % 97).zero?
              context.dispose
              next :disposed
            end
            context.eval("#{n} + 1")
          end
        rescue Expected
          next
        end
        raise "wrong result #{result.inspect}" unless result == n + 1 || OTHER_ENDS.include?(result)
      end
    end
  end.each(&:join)
  counts.sum
end

# Measured once the replacements under way have been built and their
# predecessors disposed on the pool's threads.
def settled_rss_kb
  GC.start
  sleep 1
  rss_kb
end

warm_checkouts = churn.call(seconds / 10)
warm_built = built
warm_rss = settled_rss_kb
checkouts = churn.call(seconds - (seconds / 10))
replaced = built - warm_built
abort "no contexts were replaced after the warm-up" unless replaced.positive?
rss = settled_rss_kb
pool.shutdown
limit_kb = (replaced * max_bytes / 1024) + slack_kb
puts format("%<checkouts>d checkouts by %<threads>d threads in %<seconds>s s built %<built>d contexts; " \
            "RSS %<b>d KB -> %<a>d KB after the warm-up (%<per>.1f bytes per context built), limit %<limit>d KB",
            checkouts: warm_checkouts + checkouts, threads:, seconds:, built:, b: warm_rss, a: rss,
            per: (rss - warm_rss) * 1024.0 / replaced, limit: warm_rss + limit_kb,)
abort "RSS grew #{rss - warm_rss} KB, more than #{limit_kb} KB" if rss - warm_rss > limit_kb
