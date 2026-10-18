# frozen_string_literal: true

require_relative "prelude"

# A context reading a source the process shares is taken over by a child,
# which compiles more of it lazily, reads it back and disposes of it, and the
# child's own new context, and a grandchild's, read the same inherited copy
# off their heaps. mini_racer has no shared sources: each context there keeps
# a copy of its own, and a forked child of a single-threaded parent carries
# on with an idle context as this one does.
MiniRacer::Platform.set_flags!(:single_threaded)

functions = Array.new(30_000) { |i| "function f#{i}(a) { return a + #{i} - #{i}; }" }
SOURCE = "// \u{1F600}\n#{functions.join("\n")}\nf0(1000);".freeze

def used_heap(context)
  context.low_memory_notification
  context.heap_stats[:used_heap_size]
end

def reads_the_shared_copy(own_heap)
  context = MiniRacer::Context.new
  raise "loaded" unless context.eval(SOURCE) == 1000
  raise "inner function" unless context.eval("f29999(7)") == 7

  heap = used_heap(context)
  raise "a copy of its own: #{heap} against #{own_heap}" unless heap < own_heap - SOURCE.length
end

first = MiniRacer::Context.new
raise "first" unless first.eval(SOURCE) == 1000

# A context with `max_memory:` keeps a copy of its own on its heap.
own = MiniRacer::Context.new(max_memory: 1 << 30)
raise "own" unless own.eval(SOURCE) == 1000

own_heap = used_heap(own)
own.dispose
second = MiniRacer::Context.new
raise "second" unless second.eval(SOURCE) == 1000

source_text = first.eval("f12345.toString()")

child do
  raise "taken over" unless second.eval("f29998(7)") == 7
  raise "read back" unless second.eval("f12345.toString()") == source_text

  second.dispose
  reads_the_shared_copy(own_heap)
  child { reads_the_shared_copy(own_heap) }
end
raise "parent after" unless second.eval("f29997(7)") == 7
raise "parent read back" unless second.eval("f12345.toString()") == source_text

reads_the_shared_copy(own_heap)
