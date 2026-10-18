# frozen_string_literal: true

require "test_helper"
require "json"

class SharedSourcesTest < Minitest::Test
  include RubyProcess

  # About 1.6 MB of UTF-8, which one astral character makes V8 keep two bytes
  # a unit. Each context reports its result and its heap after a collection.
  HEAPS = <<~'RUBY'
    require "json"
    require "mini_racer"
    functions = Array.new(40_000) { |i| "function f#{i}(a) { return a + #{i}; }" }
    source = "// \u{1F600}\n#{functions.join("\n")}\nf0(1);"
    contexts = Array.new(3) { MiniRacer::Context.new }
    results = contexts.map { |context| context.eval(source) }
    heaps = contexts.map do |context|
      context.low_memory_notification
      context.heap_stats[:used_heap_size]
    end
    puts JSON.generate([source.encode("UTF-16LE").bytesize, results, heaps])
  RUBY

  # The first context to load a large script keeps its source on its own
  # heap; the process has seen it by the next one, which reads it from the
  # copy every context loading it shares.
  def test_a_script_the_process_has_seen_keeps_no_copy_on_each_heap
    out, err, status = run_ruby(HEAPS, env: { "MINI_RACER_RS_CODE_CACHE" => nil })

    assert_predicate status, :success?, err
    bytes, results, (first, *later) = JSON.parse(out)

    assert_equal [1, 1, 1], results
    later.each do |heap|
      assert_operator first - heap, :>, bytes * 0.9,
                      "a later context's heap is #{first - heap} bytes smaller than the first's"
    end
  end
end
