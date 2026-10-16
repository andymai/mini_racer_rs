# frozen_string_literal: true

# A snapshot build runs in an isolate of its own, which a fork interrupts too,
# so the fork stays quick and the child takes the idle context over.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)
idle = MiniRacer::Context.new
idle.eval("var answer = 42")
builder = Thread.new { MiniRacer::Snapshot.new("var a = []; for (let i = 0; i < 3e7; i++) a.push(i % 7)") }
sleep 0.3
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
child { raise "idle" unless idle.eval("answer") == 42 }
elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
raise "fork took #{elapsed} s" if elapsed > 0.4

builder.join
