# frozen_string_literal: true

require "test_helper"
require "json"

# MiniRacerRs.heap_limit gives every context made afterwards a heap of that
# many bytes. A script that reaches it raises MiniRacerRs::HeapLimitError
# where V8 would abort the process. Each case runs in a process of its own,
# so an abort fails only that case.
module HeapLimitFixtures
  include RubyProcess

  EXECJS = File.expand_path("../../vendor/execjs/lib", __dir__)
  MIB = 1 << 20
  OBJECTS = "(() => { const a = []; for (;;) a.push({i: a.length, s: 'k' + a.length, n: [a.length, 1]}); })()"

  # Runs `code` with a limit of `mib` MiB, none for nil, and returns what it
  # printed, one JSON value per line. `outcome` gives a block's value, or the
  # class and message of what it raised.
  def capped(code, mib: 64, flags: nil, env: {}, load_path: [])
    out, err, status = run_ruby(<<~RUBY, env: env, load_path: load_path)
      require "json"
      require "mini_racer"
      $stdout.sync = true
      OBJECTS = #{OBJECTS.dump}
      #{"MiniRacer::Platform.set_flags!(#{flags})" if flags}
      #{"MiniRacerRs.heap_limit = #{mib} << 20" if mib}
      def outcome
        "ok:\#{yield.inspect}"
      rescue => e
        "\#{e.class}:\#{e.message}"
      end
      #{code}
    RUBY
    assert_predicate status, :success?, "status #{status.inspect}\nstdout:\n#{out}\nstderr:\n#{err}"
    out.lines.map { JSON.parse(_1) }
  end

  def reached(mib = 64)
    "MiniRacerRs::HeapLimitError:heap limit of #{mib * MIB} bytes reached"
  end

  def still_over(mib = 64)
    "MiniRacerRs::HeapLimitError:heap still over its limit of #{mib * MIB} bytes after a collection: " \
      "this context runs no more JavaScript"
  end
end

class HeapLimitTest < Minitest::Test
  include HeapLimitFixtures

  def test_a_script_past_the_limit_raises_and_the_context_carries_on
    lines = capped(<<~RUBY)
      context = MiniRacer::Context.new
      puts JSON.generate(context.heap_stats[:heap_size_limit])
      error = (context.eval(OBJECTS) rescue $!)
      puts JSON.generate([error.class.ancestors.take(3).map(&:name), error.message])
      puts JSON.generate(context.eval("1 + 1"))
      puts JSON.generate(context.heap_stats[:heap_size_limit])
    RUBY

    assert_equal [
      64 * MIB,
      [%w[MiniRacerRs::HeapLimitError MiniRacer::V8OutOfMemoryError MiniRacer::EvalError],
       "heap limit of #{64 * MIB} bytes reached",],
      2,
      64 * MIB,
    ], lines
  end

  def test_every_hit_on_one_context_raises
    lines = capped(<<~RUBY)
      context = MiniRacer::Context.new
      20.times { puts JSON.generate(outcome { context.eval(OBJECTS) }) }
      puts JSON.generate(outcome { context.call("Math.max", 1, 2) })
    RUBY

    assert_equal ([reached] * 20) + ["ok:2"], lines
  end

  def test_a_heap_still_over_the_limit_refuses_javascript_until_disposed
    lines = capped(<<~RUBY)
      context = MiniRacer::Context.new
      retained = "globalThis.keep = []; for (;;) keep.push({i: keep.length, s: 'k' + keep.length});"
      puts JSON.generate(outcome { context.eval(retained) })
      puts JSON.generate(outcome { context.eval("keep = null") })
      puts JSON.generate(outcome { context.call("Math.max", 1, 2) })
      puts JSON.generate(outcome { context.eval_await("Promise.resolve(1)") })
      puts JSON.generate(outcome { context.perform_microtask_checkpoint })
      puts JSON.generate(outcome { context.pump_message_loop })
      context.low_memory_notification
      puts JSON.generate(outcome { context.eval("1") })
      puts JSON.generate(context.heap_stats[:used_heap_size] > 32 << 20)
      context.dispose
      puts JSON.generate(outcome { context.eval("1") })
      puts JSON.generate(outcome { MiniRacer::Context.new.eval("1 + 1") })
    RUBY

    assert_equal [reached, *[still_over] * 6, true, "MiniRacer::ContextDisposedError:disposed context", "ok:2"], lines
  end

  # String doubling and JSON.parse grow in V8's own code, past the room a hit
  # lends at some limits; these are ones where they fit.
  def test_strings_and_json_past_the_limit_raise
    lines = capped(<<~RUBY, mib: 48)
      context = MiniRacer::Context.new
      doubling = "(() => { let s = 'abcdefghijklmno\\\\u0100'; for (;;) { s = s + s; s.charCodeAt(s.length - 1); } })()"
      appending = "(() => { const parts = []; let s = ''; for (let i = 0; ; i++) { s += 'x' + i; if (s.length > 4096) { parts.push(s); s = ''; } } })()"
      unit = '{"a":12345,"b":"xy","c":[1,2]}'
      parse = "(() => { const s = '[' + '\#{unit},'.repeat(\#{(48 << 20) * 2 / 5 / (unit.bytesize + 1)}) + '\#{unit}]'; return JSON.parse(s).length; })()"
      3.times do
        [doubling, appending, parse].each { |js| puts JSON.generate(outcome { context.eval(js) }) }
      end
      puts JSON.generate(outcome { context.eval("1 + 1") })
    RUBY

    assert_equal ([reached(48)] * 9) + ["ok:2"], lines
  end

  def test_each_hit_counts_as_heap_exhausted
    lines = capped(<<~RUBY)
      context = MiniRacer::Context.new
      2.times { context.eval(OBJECTS) rescue nil }
      puts JSON.generate(MiniRacerRs.counters(context)[:terminated])
      poisoned = MiniRacer::Context.new
      poisoned.eval("globalThis.keep = []; for (;;) keep.push({i: keep.length, s: 'k' + keep.length});") rescue nil
      poisoned.eval("1") rescue nil
      puts JSON.generate(MiniRacerRs.counters(poisoned)[:terminated][:heap_exhausted])
    RUBY

    assert_equal [{ "stop" => 0, "timeout" => 0, "interrupt" => 0, "max_memory" => 0, "heap_exhausted" => 2 }, 2],
                 lines
  end

  def test_array_buffers_are_not_held_to_the_limit
    lines = capped(<<~RUBY)
      context = MiniRacer::Context.new
      buffers = "(() => { const keep = []; for (let i = 0; i < 16; i++) keep.push(new ArrayBuffer(8 << 20)); return keep.length; })()"
      puts JSON.generate(outcome { context.eval(buffers) })
    RUBY

    assert_equal ["ok:16"], lines
  end
end

class HeapLimitPathsTest < Minitest::Test
  include HeapLimitFixtures

  def test_an_awaited_script_and_a_callback_past_the_limit_raise
    lines = capped(<<~RUBY)
      context = MiniRacer::Context.new
      awaited = "(async () => { const a = []; for (let i = 0; ; i++) { if (i % 1000 === 0) await null; a.push({i}); } })()"
      puts JSON.generate(outcome { context.eval_await(awaited) })
      nested = nil
      context.attach("allocate", proc { nested = outcome { context.eval(OBJECTS) }; 1 })
      puts JSON.generate(outcome { context.eval("allocate(); 'not reached'") })
      puts JSON.generate(nested)
      puts JSON.generate(outcome { context.eval("1 + 1") })
    RUBY

    assert_equal [reached, reached, reached, "ok:2"], lines
  end

  # A pump reports no termination, as in mini_racer; a task that reaches the
  # limit leaves the context usable.
  def test_a_task_past_the_limit_leaves_the_context_usable
    lines = capped(<<~RUBY)
      context = MiniRacer::Context.new
      context.eval("globalThis.registry = new FinalizationRegistry(() => { \#{OBJECTS} }); registry.register({}, 1)")
      context.low_memory_notification
      4.times { context.pump_message_loop }
      puts JSON.generate(outcome { context.eval("1 + 1") })
    RUBY

    assert_equal ["ok:2"], lines
  end

  # Whichever of a timeout and the limit ends a script, the next one runs.
  def test_a_timeout_racing_a_hit_ends_the_script
    lines = capped(<<~RUBY)
      (20..200).step(10) do |milliseconds|
        context = MiniRacer::Context.new(timeout: milliseconds)
        first = (context.eval(OBJECTS) rescue $!.class.name)
        puts JSON.generate([first, outcome { context.eval("1 + 1") }])
        context.dispose
      end
    RUBY

    assert_equal ["ok:2"], lines.map(&:last).uniq
    assert_empty lines.map(&:first) - %w[MiniRacer::ScriptTerminatedError MiniRacerRs::HeapLimitError]
  end

  def test_single_threaded_contexts_raise
    lines = capped(<<~RUBY, flags: ":single_threaded")
      context = MiniRacer::Context.new
      5.times { puts JSON.generate(outcome { context.eval(OBJECTS) }) }
      puts JSON.generate(outcome { context.eval("1 + 1") })
    RUBY

    assert_equal ([reached] * 5) + ["ok:2"], lines
  end

  def test_execjs_contexts_take_the_limit
    lines = capped(<<~RUBY, env: { "EXECJS_RUNTIME" => "MiniRacer" }, load_path: [EXECJS])
      require "execjs"
      context = ExecJS.compile("function allocate() { \#{OBJECTS} }")
      puts JSON.generate(outcome { context.call("allocate") })
      puts JSON.generate(outcome { context.eval("1 + 1") })
      puts JSON.generate(outcome { ExecJS.eval(OBJECTS) })
    RUBY

    assert_equal [reached, "ok:2", reached], lines
  end
end

class HeapLimitSettingTest < Minitest::Test
  include HeapLimitFixtures

  def test_nil_clears_the_limit_for_contexts_made_afterwards
    lines = capped(<<~RUBY)
      before = MiniRacer::Context.new
      puts JSON.generate(MiniRacerRs.heap_limit)
      puts JSON.generate(MiniRacerRs.heap_limit = nil)
      puts JSON.generate(MiniRacerRs.heap_limit)
      after = MiniRacer::Context.new
      puts JSON.generate([before, after].map { _1.heap_stats[:heap_size_limit] > 64 << 20 })
      puts JSON.generate(outcome { before.eval(OBJECTS) })
    RUBY

    assert_equal [64 * MIB, nil, nil, [false, true], reached], lines
  end

  # A limit of any size in range is the heap V8 reserves, less its rounding
  # to a 256 KiB page, and holds.
  def test_limits_of_any_size_are_the_heap_v8_reserves
    lines = capped(<<~RUBY, mib: nil)
      [100_000_000, 300_000_000, (127 << 20) + 1, (48 << 20) + 12_345].each do |limit|
        MiniRacerRs.heap_limit = limit
        context = MiniRacer::Context.new
        reserved = context.heap_stats[:heap_size_limit]
        puts JSON.generate([limit, reserved <= limit && reserved > limit - (256 << 10), MiniRacerRs.heap_limit])
        puts JSON.generate(outcome { context.eval(OBJECTS) })
        context.dispose
      end
    RUBY

    expected = [100_000_000, 300_000_000, (127 * MIB) + 1, (48 * MIB) + 12_345].flat_map do |limit|
      [[limit, true, limit], "MiniRacerRs::HeapLimitError:heap limit of #{limit} bytes reached"]
    end

    assert_equal expected, lines
  end

  def test_the_limit_is_an_integer_within_its_range
    lines = capped(<<~RUBY)
      ["64", 6.4e7, true, false, :x, 0, -1, (32 << 20) - 1, (4 << 30) + 1, 2**70].each do |value|
        puts JSON.generate(outcome { MiniRacerRs.heap_limit = value })
      end
      puts JSON.generate(MiniRacerRs.heap_limit)
      [32 << 20, 4 << 30].each { |value| MiniRacerRs.heap_limit = value }
      puts JSON.generate(MiniRacerRs.heap_limit)
    RUBY
    range = "ArgumentError:heap limit must be from #{32 * MIB} to #{4 << 30} bytes"

    assert_equal [
      *%w[String Float true false Symbol].map { "TypeError:wrong argument type #{_1} (expected Integer)" },
      *[range] * 5,
      64 * MIB,
      4 << 30,
    ], lines
  end

  # Flags that size the heap would make V8 ignore the limit, so neither is
  # taken once the other is set.
  def test_a_flag_that_sizes_the_heap_and_a_limit_refuse_each_other
    lines = capped(<<~RUBY, mib: nil)
      MiniRacer::Platform.set_flags!(max_old_space_size: 128)
      puts JSON.generate(outcome { MiniRacerRs.heap_limit = 64 << 20 })
      puts JSON.generate(MiniRacerRs.heap_limit)
      MiniRacer::Platform.set_flags!(max_old_space_size: 0)
      puts JSON.generate(outcome { MiniRacerRs.heap_limit = 64 << 20 })
      puts JSON.generate(outcome { MiniRacer::Platform.set_flags!("max-heap-size": 256) })
      puts JSON.generate(outcome { MiniRacer::Platform.set_flags!("no-max-semi-space-size": 1) })
      puts JSON.generate(outcome { MiniRacer::Platform.set_flags!(stack_size: "900 --max_old_space_size=8") })
      puts JSON.generate(outcome { MiniRacer::Platform.set_flags!(max_heap_size: "-1 --max_old_space_size=256") })
      puts JSON.generate(outcome { MiniRacer::Platform.set_flags!(max_old_space_size: " --max_heap_size=256") })
      puts JSON.generate(outcome { MiniRacer::Platform.set_flags!(max_old_space_size: "256 --bogus --max_old_space_size=0") })
      puts JSON.generate(outcome { MiniRacer::Platform.set_flags!(:predictable_gc_schedule) })
      puts JSON.generate(outcome { MiniRacer::Platform.set_flags!(:expose_gc, initial_heap_size: 48) })
      context = MiniRacer::Context.new
      puts JSON.generate(context.heap_stats[:heap_size_limit])
      puts JSON.generate(outcome { context.eval("typeof gc") })
    RUBY

    assert_equal [
      "ArgumentError:the heap limit would be overridden by --max_old_space_size",
      nil,
      "ok:67108864",
      "ArgumentError:--max_heap_size would override the heap limit",
      "ArgumentError:--max_semi_space_size would override the heap limit",
      "ArgumentError:--max_old_space_size would override the heap limit",
      "ArgumentError:--max_old_space_size would override the heap limit",
      "ArgumentError:--max_heap_size would override the heap limit",
      "ArgumentError:--max_old_space_size would override the heap limit",
      "ArgumentError:--predictable_gc_schedule would override the heap limit",
      "ok:nil",
      64 * MIB,
      'ok:"function"',
    ], lines
  end
end
