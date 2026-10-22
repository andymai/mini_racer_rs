# frozen_string_literal: true

# A result nested 95 deep, decoded on a fiber or a thread with from little
# more than Ruby's margin for a C function left to room a release build decodes
# it in, and then the context evaluates and disposes. A decoder that recurses until the stack runs out faults, and
# Ruby's handler then jumps over every Rust frame to raise SystemStackError,
# so nothing those frames own is freed (here the serialized result, 4 MiB
# each time) and a lookup they were inside never finishes, which hangs the
# next decode that needs it. Near its limit mini_racer 0.22.1 mostly raises
# SystemStackError from its own C recursion, but hangs when the fault lands
# inside malloc and aborts when it lands inside a collection.
require_relative "prelude"

def rss_kib
  if File.exist?("/proc/self/status")
    File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1].to_i
  else
    Integer(`ps -o rss= -p #{Process.pid}`.strip)
  end
end

check_the_dive
context = MiniRacer::Context.new
%i[fiber thread].each do |where|
  SHAPES.each do |shape, step|
    whole = context.eval(nested(step))
    answers = GAPS.map do |gap|
      answer = near_the_limit(where, gap) { outcome { context.eval(nested(step)) } }
      check_outcome(answer, "#{shape}, #{gap} levels short of a #{where}'s limit", whole)
      answer
    end
    check_decoded(answers, "#{shape} on a #{where}", whole)
  end
end
large = SHAPES.transform_values { |step| nested(step, bottom: "'x'.repeat(4 << 20)") }
at_the_limit = lambda do
  %i[fiber thread].each do |where|
    large.each do |shape, source|
      answer = near_the_limit(where, 0) { outcome { context.eval(source) } }
      check_outcome(answer, "a 4 MiB #{shape} result at a #{where}'s limit")
    end
  end
end
at_the_limit.call
before = rss_kib
3.times { at_the_limit.call }
grown = (rss_kib - before) / 1024
raise "RSS grew #{grown} MiB over #{3 * 2 * large.size} refused results of 4 MiB each" if grown > 32

usable(context)
