# frozen_string_literal: true

# Threads that each create a context, evaluate on it and dispose it, over and
# over for a while; fails on any wrong result or exception.
#   ruby -Ilib test/stress/churn.rb [threads] [seconds]

require "mini_racer"

threads = Integer(ARGV.fetch(0, 8))
seconds = Float(ARGV.fetch(1, 600))
deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + seconds
Thread.abort_on_exception = true

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
workers.each(&:join)
GC.start
puts "#{counts.sum} contexts created, evaluated and released by #{threads} threads in #{seconds} s"
