# frozen_string_literal: true

# Forks over and over with a single-threaded V8: first with every context idle,
# then while another thread evaluates in a loop. Each child uses an inherited
# context and a new one and must exit cleanly within three seconds; the parent
# checks every context after each fork. Fails on the first failed or hung child.
#   ruby -Ilib test/stress/fork_loop.rb [quiet forks] [busy forks]

require "mini_racer"

MiniRacer::Platform.set_flags!(:single_threaded)

quiet = Integer(ARGV.fetch(0, 500))
busy = Integer(ARGV.fetch(1, 100))
Thread.abort_on_exception = true

def fork_once(context, count)
  pid = fork do
    Thread.new do
      sleep 3
      warn "child timed out at count #{count}"
      exit! 99
    end
    exit!(1) unless context.eval("++count") == count + 1
    fresh = MiniRacer::Context.new
    exit!(2) unless fresh.eval("6 * 7") == 42
    fresh.dispose
    context.low_memory_notification if context.respond_to?(:low_memory_notification)
    exit!(0)
  end
  _, status = Process.wait2(pid)
  raise "child failed at count #{count}: #{status.inspect}" unless status.success?
end

contexts = Array.new(3) { MiniRacer::Context.new.tap { |context| context.eval("var count = 0") } }
quiet.times do |round|
  fork_once(contexts[round % contexts.size], contexts[round % contexts.size].eval("count"))
  contexts.each { |context| raise "parent context" unless context.eval("count").is_a?(Integer) }
end

looping = MiniRacer::Context.new
stop = false
worker = Thread.new do
  n = 0
  n += 1 while !stop && looping.eval("#{n} + 1") == n + 1
  raise "worker stopped early" unless stop
end
busy.times do |round|
  fork_once(contexts[round % contexts.size], contexts[round % contexts.size].eval("count"))
end
stop = true
worker.join
puts "#{quiet} forks with idle contexts and #{busy} with an eval loop running, all children clean"
