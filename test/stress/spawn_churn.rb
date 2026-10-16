# frozen_string_literal: true

# Child processes through backticks and IO.popen while four threads create,
# evaluate and dispose contexts. A spawned child runs no Ruby, so no fork
# waits for the runners: nothing may hang, and every spawn must return within
# a second. The V8 platform mode is fixed per process, so each mode is its own
# run.
#   ruby -Ilib test/stress/spawn_churn.rb single|multi [spawns]

require "mini_racer"

mode = ARGV.fetch(0) { abort "usage: spawn_churn.rb single|multi [spawns]" }
abort "mode is single or multi, not #{mode}" unless %w[single multi].include?(mode)
spawns = Integer(ARGV.fetch(1, 1000))
MiniRacer::Platform.set_flags!(:single_threaded) if mode == "single"
Thread.abort_on_exception = true

def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

stop = false
evals = Array.new(4, 0)
churners = Array.new(4) do |n|
  Thread.new do
    until stop
      context = MiniRacer::Context.new
      raise "churn" unless context.eval("#{n} + 1") == n + 1

      context.dispose
      evals[n] += 1
    end
  end
end

slowest = 0
spawns.times do |i|
  started = now
  out = i.even? ? `echo #{i}` : IO.popen(["echo", i.to_s], &:read)
  raise "spawn #{i} printed #{out.inspect}" unless out.strip == i.to_s

  took = now - started
  raise "spawn #{i} took #{took} s" if took > 1

  slowest = took if took > slowest
end
stop = true
churners.each(&:join)
puts "#{spawns} spawns, #{evals.sum} contexts churned, #{mode}-threaded V8; slowest spawn #{slowest.round(3)} s"
