# frozen_string_literal: true

# A forked child evaluates the single-threaded context it took over. With the
# madvise wrapper it exits 0; built without it (scripts/madvise-fixture.sh)
# the child dies by a signal, because V8 maps its memory MADV_DONTFORK and the
# child has none of it.
#   ruby -Ilib test/stress/madvise_fixture.rb clean|crash

require "mini_racer"

expect = ARGV.fetch(0) { abort "usage: madvise_fixture.rb clean|crash" }
abort "expect clean or crash, not #{expect}" unless %w[clean crash].include?(expect)

MiniRacer::Platform.set_flags!(:single_threaded)
context = MiniRacer::Context.new
context.eval("var answer = 42")
pid = fork { exit!(context.eval("answer") == 42 ? 0 : 1) }
_, status = Process.wait2(pid)
abort "the parent's context stopped working" unless context.eval("answer") == 42

outcome = status.signaled? ? "crash" : "clean"
abort "expected the child to #{expect == "crash" ? "crash" : "exit 0"}, got #{status.inspect}" unless
  outcome == expect && (expect == "crash" || status.success?)
puts "child: #{status.inspect}, as expected"
