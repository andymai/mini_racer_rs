# frozen_string_literal: true

# A result nested 95 deep, decoded on a fiber or a thread with little more
# than Ruby's margin for a C function left, and then the context evaluates and
# disposes. A decoder that recurses until the stack runs out faults, and
# Ruby's handler then jumps over every Rust frame to raise SystemStackError,
# so nothing those frames own is freed (here the serialized result, 4 MiB
# each time) and a lookup they were inside never finishes, which hangs the
# next decode that needs it. Near its limit mini_racer 0.22.1 mostly raises
# SystemStackError from its own C recursion, but hangs when the fault lands
# inside malloc and aborts when it lands inside a collection.
require_relative "prelude"

def rss_kib = Integer(`ps -o rss= -p #{Process.pid}`.strip)

check_the_dive
context = MiniRacer::Context.new
%i[fiber thread].each do |where|
  SHAPES.each_value do |step|
    GAPS.each do |gap|
      near_the_limit(where, gap) { outcome { context.eval(nested(step)) } }
    end
  end
end
large = nested("v = {a: v}", bottom: "'x'.repeat(4 << 20)")
2.times { near_the_limit(:fiber, 0) { outcome { context.eval(large) } } }
before = rss_kib
20.times { near_the_limit(:fiber, 0) { outcome { context.eval(large) } } }
grown = (rss_kib - before) / 1024
raise "RSS grew #{grown} MiB over 20 results of 4 MiB each" if grown > 32

usable(context)
