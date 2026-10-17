# frozen_string_literal: true

# Forks over and over with the default multi-threaded V8 while other threads
# evaluate, as a preloaded server that forks from a busy worker does: one
# thread through an attached Ruby callback, one with ensure_gc_after_idle and
# short pauses, the rest plain. Each child makes a new context, runs
# JavaScript hot enough for V8 to hand compilation to its worker threads, and
# must exit cleanly within three seconds, or it is killed. Fails on the first
# failed or hung child.
#   ruby -Ilib test/stress/fork_busy.rb [forks] [threads]

require "mini_racer"

forks = Integer(ARGV.fetch(0, 300))
threads = Integer(ARGV.fetch(1, 4))
Thread.abort_on_exception = true

CHILD_DEADLINE = 3
WORK = <<~JS
  function work(n) {
    const rows = [];
    for (let i = 0; i < n; i++) rows.push({ i, label: "row " + i, even: i % 2 === 0 });
    return rows.filter((row) => row.even).map((row) => row.label).join(",").length;
  }
JS

def reap(pid, round)
  deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + CHILD_DEADLINE
  loop do
    _, status = Process.wait2(pid, Process::WNOHANG)
    return status if status

    if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
      Process.kill(:KILL, pid)
      Process.wait(pid)
      raise "child hung at fork #{round}"
    end
    sleep 0.001
  end
end

stop = false
busy = Array.new(threads) do |i|
  Thread.new do
    case i
    when 0
      context = MiniRacer::Context.new
      context.eval(WORK)
      context.attach("rows", proc { |n| n * 2 })
      context.eval("work(rows(1000))") until stop
    when 1
      context = MiniRacer::Context.new(ensure_gc_after_idle: 1)
      context.eval(WORK)
      until stop
        context.eval("work(2000)")
        sleep 0.002
      end
    else
      context = MiniRacer::Context.new
      context.eval(WORK)
      context.eval("work(2000)") until stop
    end
  end
end
sleep 0.5

forks.times do |round|
  pid = fork do
    fresh = MiniRacer::Context.new
    fresh.eval(WORK)
    20.times { exit!(2) unless fresh.eval("work(2000)").positive? }
    fresh.dispose
    exit!(0)
  end
  status = reap(pid, round)
  raise "child failed at fork #{round}: #{status.inspect}" unless status.success?
end
stop = true
busy.each(&:join)
puts "#{forks} forks with #{threads} threads evaluating, all children clean"
