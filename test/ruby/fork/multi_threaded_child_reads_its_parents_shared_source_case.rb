# frozen_string_literal: true

require_relative "prelude"

# A multi-threaded child refuses the contexts it inherits, and never frees
# them or the strings they hold, so the source they read stays where it is;
# a new context of the child loading the same script reads that inherited
# copy rather than one of its own. mini_racer's contexts each keep a copy.
functions = Array.new(30_000) { |i| "function f#{i}(a) { return a + #{i} - #{i}; }" }
SOURCE = "// \u{1F600}\n#{functions.join("\n")}\nf0(1000);".freeze

def used_heap(context)
  context.low_memory_notification
  context.heap_stats[:used_heap_size]
end

first = MiniRacer::Context.new
raise "first" unless first.eval(SOURCE) == 1000

own_heap = used_heap(first)
second = MiniRacer::Context.new
raise "second" unless second.eval(SOURCE) == 1000

child do
  raise "inherited" unless raised(MiniRacer::RuntimeError) { second.eval("1") }

  2.times do
    fresh = MiniRacer::Context.new
    raise "loaded" unless fresh.eval(SOURCE) == 1000
    raise "inner function" unless fresh.eval("f29999(7)") == 7

    heap = used_heap(fresh)
    raise "a copy of its own: #{heap} against #{own_heap}" unless heap < own_heap - SOURCE.length

    fresh.dispose
  end
end
raise "parent after" unless second.eval("f29999(7)") == 7
