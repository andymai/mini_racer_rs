# frozen_string_literal: true

# A throw out of a callback leaves its thread waiting for the runner with the
# GVL held; the forks meanwhile must neither deadlock nor lose a throw.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)
context = MiniRacer::Context.new
context.attach("leave", proc { throw :out })
throws = 0
stop = false
thrower = Thread.new do
  until stop
    catch(:out) { context.eval("leave()") }
    throws += 1
  end
end
50.times { child { exit! 0 } }
stop = true
thrower.join
raise "no throws" if throws.zero?
