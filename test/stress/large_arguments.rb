# frozen_string_literal: true

# ExecJS calls whose arguments keep every source the same length past 16,383
# characters, beyond which V8 hashes a string by its length alone. The normal
# path, which MINI_RACER_RS_FAST_CALLS=off takes, compiles each source, and
# V8's compilation cache compares it with every earlier one of that length: on
# x86_64 Linux a call with 24 KB of arguments takes 0.57 ms there at first and
# 19 ms after 3,000 calls. Fails when the median of the last tenth of the
# calls is over `max_ratio` times the median of the first tenth.
#   ruby -Ilib -Ivendor/execjs/lib test/stress/large_arguments.rb [calls] [max_ratio]

require "mini_racer"
require "execjs"

calls = Integer(ARGV.fetch(0, 3000))
max_ratio = Float(ARGV.fetch(1, 3))
context = ExecJS::Runtimes::MiniRacer.compile("function len(s) { return s.length }")
filler = "x" * 24_000
times = Array.new(calls) do |i|
  argument = filler + format("%06d", i)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  context.call("len", argument)
  Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
end
window = [calls / 10, 1].max
median = ->(list) { list.sort[list.size / 2] * 1e6 }
first = median.call(times.first(window))
last = median.call(times.last(window))
puts "#{calls} calls with #{filler.size + 6}-character arguments: median #{first.round(1)} us over the first " \
     "#{window}, #{last.round(1)} us over the last #{window}"
abort "the last calls took #{(last / first).round(1)} times as long as the first" if last > first * max_ratio
