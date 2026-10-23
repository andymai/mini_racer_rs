# frozen_string_literal: true

# Loads this gem for a fork case and gives it `child`, which forks, runs the
# block in the child and fails the case unless the child exits cleanly within
# five seconds or the `within:` it is given; `reap`, which waits for a child
# and kills it after; `raised`, the message of the error a block raises; and
# `skip_case`, which exits with the code ForkTest reports as a skip, or fails
# under CI so a leg missing a prerequisite cannot drop the case without a word.

$LOAD_PATH.unshift File.expand_path("../../../lib", __dir__)
require "mini_racer"

def child(within: 5, &)
  pid = fork do
    Thread.new do
      sleep within
      warn "child timed out"
      exit! 99
    end
    yield
    exit! 0
  rescue Exception => e # rubocop:disable Lint/RescueException
    warn "child: #{e.class}: #{e.message}"
    exit! 1
  end
  # A second past the child's own deadline, so a slow child reports itself.
  status = reap(pid, within: within + 1)
  raise "child failed: #{status.inspect}" unless status.success?
end

def reap(pid, within: 5)
  deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + within
  loop do
    _, status = Process.wait2(pid, Process::WNOHANG)
    return status if status

    if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
      Process.kill(:KILL, pid)
      Process.wait(pid)
      raise "child #{pid} hung"
    end
    sleep 0.01
  end
end

SKIP_EXIT = 77

def skip_case(reason)
  puts reason
  $stdout.flush
  exit!(ENV["CI"] ? 1 : SKIP_EXIT)
end

def raised(klass)
  yield
  nil
rescue klass => e
  e.message
end
