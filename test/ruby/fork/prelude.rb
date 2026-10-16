# frozen_string_literal: true

# Loads this gem for a fork case and gives it `child`, which forks, runs the
# block in the child and fails the case unless the child exits cleanly within
# five seconds, and `raised`, the message of the error a block raises.

$LOAD_PATH.unshift File.expand_path("../../../lib", __dir__)
require "mini_racer"

def child(&)
  pid = fork do
    Thread.new do
      sleep 5
      warn "child timed out"
      exit! 99
    end
    yield
    exit! 0
  rescue Exception => e # rubocop:disable Lint/RescueException
    warn "child: #{e.class}: #{e.message}"
    exit! 1
  end
  _, status = Process.wait2(pid)
  raise "child failed: #{status.inspect}" unless status.success?
end

def raised(klass)
  yield
  nil
rescue klass => e
  e.message
end
