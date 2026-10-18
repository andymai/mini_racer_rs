# frozen_string_literal: true

require "test_helper"
require "digest"
require "json"
require "mini_racer"

# ExecJS's `call` and react-rails' renders send a new source per call, which
# the engine runs as a template compiled once per context. Every expected
# value in these tests is what mini_racer 0.22.1 gives for the same source.
module FastCallsFixtures
  EXECJS = File.expand_path("../../vendor/execjs/lib", __dir__)

  SOURCE = <<~JS
    function add(a, b) { return a + b }
    function inner(x) { throw new Error("boom " + JSON.stringify(x)) }
    function outer(x) { return inner(x) }
    function stack() { return new Error("s").stack }
    function receiver() { return this === globalThis }
    function keys(o) { return [Object.keys(o), Object.getPrototypeOf(o) === Object.prototype] }
    var notfn = 1;
    var ns = { deep: { fn: function (a) { return [this === ns.deep, a] } } };
    let lexical = (a) => a * 3;
    var console = { history: [] };
    ["error", "log"].forEach(function (level) {
      console[level] = function () { console.history.push({ level: level, arguments: Array.prototype.slice.call(arguments) }) };
    });
    var ReactRailsUJS = {
      serverRender: function (fn, name, props) {
        if (name === "Throws") throw new TypeError("bad " + props.n);
        if (name === "Cyclic") { var o = {}; o.o = o; console.log(o) }
        if (name === "Logs") console.log("hi", props.n);
        return "<" + fn + ":" + name + ">" + JSON.stringify(props);
      },
    };
  JS

  # Code a renderer runs around each render, here collecting what the render
  # logged into the HTML, which react-rails does with its console replay.
  BEFORE = <<~JS
    var logged = [];
    console.history = logged;
  JS
  AFTER = <<~JS
    if (logged.length > 0) {
      result += "\\n<!--" + logged.map(function (entry) {
        return " " + entry.level + " " + JSON.stringify(entry.arguments);
      }).join("") + " -->";
    }
  JS

  # What reflection shows of the bottom frame: its script's hash and raw
  # offset. A render also returns the wrapper's source through `caller`, and
  # its arguments, and keeps the wrapper through `caller` and
  # `CallSite#getFunction` for `rerun` to call again.
  REFLECTING = <<~JS
    function stack(read) {
      var saved = Error.prepareStackTrace;
      Error.prepareStackTrace = function (e, frames) { return read(frames) };
      try { return new Error().stack } finally { Error.prepareStackTrace = saved }
    }
    function hash() { return stack(function (frames) { return frames[frames.length - 1].getScriptHash() }) }
    function position() { return stack(function (frames) { return frames[frames.length - 1].getPosition() }) }
    function add(a, b) { return a + b }
    var ReactRailsUJS = {
      serverRender: function (fn, name, props) {
        globalThis.byCaller = ReactRailsUJS.serverRender.caller;
        globalThis.byFunction = stack(function (frames) { return frames[2].getFunction() });
        return [hash(), position(), String(byCaller), [fn, name, props]];
      },
    };
    function rerun(kept) { return globalThis[kept].call(globalThis)[3] }
  JS

  THROWN = ["Error: boom {\"k\":1}", ["inner (<eval>:2:27)", "outer (<eval>:3:28)", "<eval>:1:8"]].freeze
  RENDER_THROWN = ["TypeError: bad 3", ["Object.serverRender (<eval>:16:34)", "<eval>:5:45", "<eval>:13:13"]].freeze

  module_function

  # react-rails' ExecJSRenderer#compose_js, which ExecJS's eval wraps in
  # parentheses.
  def compose_js(before, main, after)
    <<-JS
          (function () {
            #{before}
            var result = #{main};
            #{after}
            return result;
          })()
    JS
  end

  def render(component, props, before: BEFORE, after: AFTER)
    main = "this.ReactRailsUJS.serverRender('renderToString', '#{component}', #{props})"
    compose_js(before, main, after)
  end

  # The message and the JavaScript frames ExecJS puts ahead of Ruby's.
  def failure
    yield
    raise "nothing raised"
  rescue ExecJS::Error => e
    [e.message, e.backtrace.take_while { |line| !line.match?(/\.rb:\d+/) }]
  end
end

class FastCallsTest < Minitest::Test
  include FastCallsFixtures

  def setup
    raise "vendor/execjs is empty; run `git submodule update --init`" unless File.file?(File.join(EXECJS, "execjs.rb"))

    $LOAD_PATH.unshift(EXECJS) unless $LOAD_PATH.include?(EXECJS)
    require "execjs"
    @context = ExecJS::Runtimes::MiniRacer.compile(SOURCE)
  end

  def test_calls_return_what_mini_racer_returns
    value = { "a" => [1, nil, true, 1.5, "\u00e9\u2028"] }

    assert_equal 3, @context.call("add", 1, 2)
    assert_equal [false, value], @context.call("ns.deep.fn", value)
    assert @context.call("receiver")
    assert_equal 6, @context.call("lexical", 2)
    assert_equal "Error: s\n    at stack (<eval>:4:27)\n    at <eval>:1:8", @context.call("stack")
  end

  def test_a_proto_key_stays_an_own_property_as_json_makes_it
    assert_equal [["a"], false], @context.call("keys", { "__proto__" => { "x" => 1 }, "a" => 1 })
  end

  def test_call_errors_carry_mini_racers_messages_and_frames
    missing = "TypeError: Cannot read properties of undefined (reading 'apply')"

    assert_equal(["ReferenceError: nope is not defined", ["<eval>:1:1"]], failure { @context.call("nope", 1) })
    assert_equal(["TypeError: notfn.apply is not a function", ["<eval>:1:8"]], failure { @context.call("notfn", 1) })
    assert_equal([missing, ["<eval>:1:13"]], failure { @context.call("ns.missing", 1) })
    assert_equal(THROWN, failure { @context.call("outer", { "k" => 1 }) })
    assert_raises(ExecJS::ProgramError) { @context.call("nope", 1) }
  end

  def test_renders_return_what_mini_racer_returns
    assert_equal "<renderToString:Hello>{\"name\":\"W\u00e9\"}",
                 @context.eval(render("Hello", { "name" => "W\u00e9" }.to_json))
    assert_equal "<renderToString:Logs>{\"n\":3}\n<!-- log [\"hi\",3] -->", @context.eval(render("Logs", '{"n":3}'))
    assert_equal "<renderToString:Logs>{\"n\":3}", @context.eval(render("Logs", '{"n":3}', before: "", after: ""))
  end

  def test_render_errors_carry_mini_racers_frames_before_and_after_the_props
    circular = [
      "JavaScript --> starting at object with constructor 'Object'",
      "JavaScript --- property 'o' closes the circle",
      "JSON.stringify ((execjs))", "<eval>:8:43", "Array.map ((execjs))", "<eval>:7:31", "<eval>:13:13",
    ]
    cyclic = failure { @context.eval(render("Cyclic", "{}")) }

    assert_equal(RENDER_THROWN, failure { @context.eval(render("Throws", '{"n":3}')) })
    assert_equal ["TypeError: Converting circular structure to JSON", circular], cyclic
  end

  def test_props_that_are_not_json_on_one_line_render_as_mini_racer_renders_them
    pretty = ["Object.serverRender (<eval>:16:34)", "<eval>:5:45", "<eval>:15:13"]

    assert_equal(["TypeError: bad 4", pretty], failure { @context.eval(render("Throws", "{\n  \"n\": 4\n}")) })
    assert_equal "<renderToString:Hello>{\"n\":\"js1\"}", @context.eval(render("Hello", "{n: 'js' + 1}"))
    assert_equal "<renderToString:Hello>{\"b\":2}", @context.eval(render("Hello", '{"__proto__":{"a":1},"b":2}'))
  end
end

class FastCallsContextTest < Minitest::Test
  include FastCallsFixtures

  def test_a_call_made_while_another_reads_its_path_leaves_each_its_own_arguments
    context = MiniRacer::Context.new
    context.attach("rubyEval", proc { |source| context.eval(source) })
    context.eval(<<~JS)
      function add(a, b) { return a + b }
      var log = [];
      var obj = {};
      Object.defineProperty(obj, "fn", {
        get: function () { log.push(rubyEval("(add.apply(this, [100,1]))")); return function (a) { return [a, log] } }
      });
      function nested(v) { return [v, rubyEval("(add.apply(this, [" + v + ",1]))")] }
    JS

    assert_equal ["outer", [101]], context.eval('(obj.fn.apply(this, ["outer"]))')
    assert_equal [5, 6], context.eval("(nested.apply(this, [5]))")
  ensure
    context&.dispose
  end

  # A source sent again runs as mini_racer runs it, from V8's compilation
  # cache, down to what reflection shows of its frames: the script's hash and
  # raw offsets past the values it carries.
  def test_a_source_sent_again_runs_as_mini_racer_runs_it
    context = MiniRacer::Context.new
    context.eval(REFLECTING)
    call = "(hash.apply(this, [1]))"
    rendered = Array.new(3) { context.eval("(#{render("Hello", '{"n":1}', before: "", after: "")})") }

    hashes = Array.new(3) { context.eval(call) }.drop(1)
    offsets = rendered.drop(1).map { |(_, offset)| offset }

    assert_equal [Digest::SHA256.hexdigest(call)] * 2, hashes
    assert_equal [185, 185], offsets
  ensure
    context&.dispose
  end

  def test_calls_to_more_functions_than_a_context_keeps_templates_for_all_return
    context = MiniRacer::Context.new
    context.eval(Array.new(40) { |i| "function f#{i}(a) { return a + #{i} }" }.join("\n"))
    results = Array.new(80) { |i| context.eval("(f#{i % 40}.apply(this, [1]))") }

    assert_equal Array.new(80) { |i| (i % 40) + 1 }, results
  ensure
    context&.dispose
  end

  def test_a_terminated_call_raises_as_mini_racer_raises_and_the_context_goes_on
    context = MiniRacer::Context.new(timeout: 100)
    context.eval("function spin() { for (;;) {} }\nfunction add(a, b) { return a + b }")
    error = assert_raises(MiniRacer::ScriptTerminatedError) { context.eval("(spin.apply(this, [1]))") }

    assert_equal "terminated", error.message
    assert_equal 3, context.eval("(add.apply(this, [1,2]))")
  ensure
    context&.dispose
  end

  def test_a_call_over_max_memory_raises_as_mini_racer_raises
    context = MiniRacer::Context.new(max_memory: 20_000_000)
    context.eval("function hog() { var a = []; for (;;) a.push(new Array(100000).fill(1)) }")
    error = assert_raises(MiniRacer::V8OutOfMemoryError) { context.eval("(hog.apply(this, []))") }

    assert_equal "out of memory", error.message
  ensure
    context&.dispose
  end
end

# Code around a render's call can run it other than once at the wrapper's top
# level. Each test renders its code with two props: the first render compiles
# the template, or finds it refused, and the second hits that template. Both
# render as mini_racer renders them.
class FastCallsWrapperTest < Minitest::Test
  include FastCallsFixtures

  def setup
    @context = MiniRacer::Context.new
    @context.attach("rubyEval", proc { |source| @context.eval(source) })
    @context.eval("var ReactRailsUJS = { serverRender: function (fn, name, props) { return [fn, name, props] } };")
  end

  def teardown
    @context&.dispose
  end

  def wrapped(before, after, props)
    "(#{compose_js(before, "this.ReactRailsUJS.serverRender('renderToString', 'A', #{props.to_json})", after)})"
  end

  def test_a_template_literal_around_the_call_holds_its_text
    [1, 2].each do |n|
      line = "\n            var result = this.ReactRailsUJS.serverRender('renderToString', 'A', " \
             "{\"n\":#{n}});\n            "

      assert_equal line, @context.eval(wrapped("var s = `", "`; var result = s;", { "n" => n }))
    end
  end

  def test_a_loop_around_the_call_makes_props_on_each_pass
    before = "var seen = []; for (var i = 0; i < 2; i++) {"
    after = "seen.push(result); } result = seen[0][2] === seen[1][2];"
    results = [1, 2].map { |n| @context.eval(wrapped(before, after, { "n" => n })) }

    assert_equal [false, false], results
  end

  def test_a_call_left_for_a_later_script_keeps_its_arguments
    [1, 2].each do |n|
      deferred = wrapped("var result; globalThis.later = function () {", "return result; };", { "n" => n })

      assert_nil @context.eval(deferred)
      assert_equal ["renderToString", "A", { "n" => n }], @context.eval("later()")
    end
  end

  def test_a_call_a_promise_makes_after_a_nested_render_keeps_its_arguments
    before = "var result; Promise.resolve().then(function () { globalThis.got = (function () {"
    [1, 2].each do |n|
      source = wrapped(before, "return result; })(); });", { "n" => n })

      assert_equal 0, @context.eval("rubyEval(#{source.to_json}); 0")
      assert_equal ["renderToString", "A", { "n" => n }], @context.eval("got")
    end
  end

  def test_a_tagged_template_is_new_for_each_source
    before = "function tag(strings) { return strings }\nvar strings = tag`x`;"
    after = "if (!globalThis.kept) globalThis.kept = strings;\nresult = globalThis.kept === strings;"
    results = [1, 2].map { |n| @context.eval(wrapped(before, after, { "n" => n })) }

    assert_equal [true, false], results
  end

  def test_the_wrapper_called_again_through_arguments_keeps_its_arguments
    ["globalThis.again = arguments.callee;", "globalThis.again = eval('argu' + 'ments').callee;"].each do |before|
      1.upto(2) do |n|
        expected = ["renderToString", "A", { "n" => n }]

        assert_equal expected, @context.eval(wrapped(before, "", { "n" => n }))
        assert_equal expected, @context.eval("again()")
      end
    end
  end

  # A `.` that ends a line comment or a number reads as no property access
  # past the line's end, so the `arguments` after it is the wrapper's own.
  def test_arguments_after_a_dot_on_the_line_before_renders_again_with_new_props
    @context.eval("ReactRailsUJS.serverRender = function (fn, name, props) { return ++props.n };")
    ["// render once more.", "var x = 1."].each do |line|
      before = "if (!globalThis.again) {\n  globalThis.again = true;\n  #{line}\n  " \
               "arguments.callee.call(this);\n  globalThis.again = false;\n}"
      results = 1.upto(2).map { |n| @context.eval(wrapped(before, "", { "n" => n })) }

      assert_equal [2, 3], results, line
    end
  end
end

class FastCallsProcessTest < Minitest::Test
  include RubyProcess
  include FastCallsFixtures

  OUTCOMES = <<~RUBY.freeze
    require "json"
    require "mini_racer"
    require "execjs"
    context = ExecJS::Runtimes::MiniRacer.compile(#{SOURCE.dump})
    outcome = lambda do |&block|
      block.call
    rescue ExecJS::Error => e
      [e.message, e.backtrace.take_while { |line| !line.match?(/\\.rb:\\d+/) }]
    end
    inner = context.instance_variable_get(:@context)
    inner.low_memory_notification
    before = inner.heap_stats[:used_heap_size]
    40_000.times { |i| context.call("add", i, 2) }
    puts JSON.generate([
      outcome.call { context.call("add", 1, 2) },
      outcome.call { context.call("outer", { "k" => 1 }) },
      outcome.call { context.eval(#{FastCallsFixtures.render("Throws", '{"n":3}').dump}) },
      inner.heap_stats[:used_heap_size] - before < 8_000_000,
    ])
  RUBY

  # The normal path compiles each source to a script V8 keeps until a full
  # collection, about 30 MB of heap over 40,000 of these calls and 97 MB over
  # as many renders.
  def test_calls_and_renders_keep_no_compiled_script_each
    head, tail = "(#{render("Hello", "PROPS")})".split("PROPS")
    out, err, status = run_ruby(<<~RUBY, env: { "MINI_RACER_RS_FAST_CALLS" => nil })
      require "mini_racer"
      context = MiniRacer::Context.new
      context.eval(#{SOURCE.dump})
      grown = lambda do |&step|
        context.low_memory_notification
        before = context.heap_stats[:used_heap_size]
        40_000.times(&step)
        context.heap_stats[:used_heap_size] - before
      end
      calls = grown.call { |i| context.eval("(add.apply(this, [" + i.to_s + ",2]))") }
      renders = grown.call { |i| context.eval(#{head.dump} + '{"n":' + i.to_s + "}" + #{tail.dump}) }
      puts calls, renders
    RUBY
    calls, renders = out.split.map { Integer(_1) }

    assert_predicate status, :success?, err
    assert_operator calls, :<, 8_000_000, "the heap grew #{calls} bytes over 40,000 calls"
    assert_operator renders, :<, 8_000_000, "the heap grew #{renders} bytes over 40,000 renders"
  end

  def test_the_off_switch_gives_the_same_outcomes_through_the_normal_path
    { nil => true, "off" => false }.each do |switch, flat|
      out, err, status = run_ruby(OUTCOMES, env: { "MINI_RACER_RS_FAST_CALLS" => switch }, load_path: [EXECJS])

      assert_predicate status, :success?, err
      assert_equal [3, THROWN, RENDER_THROWN, flat], JSON.parse(out), "MINI_RACER_RS_FAST_CALLS=#{switch.inspect}"
    end
  end

  def test_use_strict_gives_mini_racers_receivers_and_frames
    out, err, status = run_ruby(<<~RUBY, load_path: [EXECJS])
      require "json"
      require "mini_racer"
      require "execjs"
      MiniRacer::Platform.set_flags!(:use_strict)
      context = ExecJS::Runtimes::MiniRacer.compile(#{SOURCE.dump})
      begin
        context.eval(#{render("Hello", "{}").dump})
      rescue ExecJS::ProgramError => e
        rendered = [e.message, e.backtrace.take_while { |line| !line.match?(/\\.rb:\\d+/) }]
      end
      puts JSON.generate([context.call("receiver"), context.call("stack"), rendered])
    RUBY
    rendered = ["TypeError: Cannot read properties of undefined (reading 'ReactRailsUJS')",
                ["<eval>:5:31", "<eval>:13:13"],]

    assert_predicate status, :success?, err
    assert_equal [true, "Error: s\n    at stack (<eval>:4:27)\n    at <eval>:1:8", rendered], JSON.parse(out)
  end

  # mini_racer's verbose TryCatch prints the offset in the script where an
  # exception happened, past the props here.
  def test_verbose_exceptions_report_the_offset_mini_racer_reports
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      context = MiniRacer::Context.new(verbose_exceptions: true)
      context.eval(#{SOURCE.dump})
      begin
        context.eval(#{"(#{render("Cyclic", '{"n":1}')})".dump})
      rescue MiniRacer::RuntimeError
        nil
      end
    RUBY

    assert_predicate status, :success?, err
    assert_equal "<eval>:309: Uncaught TypeError: Converting circular structure to JSON\n    " \
                 "--> starting at object with constructor 'Object'\n    " \
                 "--- property 'o' closes the circle\n",
                 out
  end
end

class FastCallsReaderTest < Minitest::Test
  include RubyProcess
  include FastCallsFixtures

  # Fast calls read their values through a const, which code that finds its
  # name cannot assign, delete or shadow.
  def test_code_that_finds_the_reader_cannot_break_later_fast_calls
    first, second = [1, 2].map { |n| "(#{render("Hello", "{\"n\":#{n}}", before: "", after: "")})" }
    out, err, status = run_ruby(<<~RUBY, env: { "MINI_RACER_RS_FAST_CALLS" => nil })
      require "json"
      require "mini_racer"
      context = MiniRacer::Context.new
      context.eval(#{REFLECTING.dump})
      run = lambda do |source|
        context.eval(source)
      rescue MiniRacer::Error => e
        e.message
      end
      reader = run.call(#{first.dump})[2][/__mini_racer_rs_\\h+/]
      assigned = run.call("(0, eval)('" + reader + " = null')")
      run.call("delete " + reader + "; globalThis['" + reader + "'] = null")
      sums = [run.call("(add.apply(this, [1,2]))"), run.call("(add.apply(this, [3,4]))")]
      rendered = run.call(#{second.dump})
      puts JSON.generate([assigned, sums, rendered.is_a?(Array) && rendered[2].include?(reader)])
    RUBY

    assert_predicate status, :success?, err
    assert_equal ["TypeError: Assignment to constant variable.", [3, 7], true], JSON.parse(out)
  end
end

class FastCallsCapTest < Minitest::Test
  include RubyProcess
  include FastCallsFixtures

  # A template is kept per context, so one over 1 MiB takes the normal path.
  # Its length is the source's, less the literals' 30 bytes, plus the three
  # reads of the reader the first render shows.
  def test_a_template_of_1_mib_is_a_fast_call_and_one_byte_more_is_not
    wrapped = "(#{render("Hello", '{"n":1}', before: "BEFORE", after: "")})"
    out, err, status = run_ruby(<<~RUBY, env: { "MINI_RACER_RS_FAST_CALLS" => nil })
      require "digest"
      require "json"
      require "mini_racer"
      context = MiniRacer::Context.new
      context.eval(#{REFLECTING.dump})
      wrapped = ->(before) { #{wrapped.dump}.sub("BEFORE", before) }
      reader = context.eval(wrapped.call(""))[2][/__mini_racer_rs_\\h+/]
      template = ->(source) { source.bytesize - 30 + (3 * (reader.size + 3)) }
      padded = ->(size) { wrapped.call("var pad = 0;" + (" " * (size - template.call(wrapped.call("var pad = 0;"))))) }
      sources = [padded.call(1 << 20), padded.call((1 << 20) + 1)]
      puts JSON.generate(sources.map { |source| [template.call(source), context.eval(source)[0] == Digest::SHA256.hexdigest(source)] })
    RUBY

    assert_predicate status, :success?, err
    assert_equal [[1 << 20, false], [(1 << 20) + 1, true]], JSON.parse(out)
  end
end

class FastCallsDeviationTest < Minitest::Test
  include RubyProcess
  include FastCallsFixtures

  # An ExecJS call before and after two renders with different props, the
  # first compiling the template and the second hitting it, then the wrapper
  # the renders kept called again outside a fast call and during one.
  REFLECTED = <<~RUBY.freeze
    require "digest"
    require "json"
    require "mini_racer"
    context = MiniRacer::Context.new
    context.eval(#{REFLECTING.dump})
    hashes_source = lambda do |source|
      hash, offset, wrapper, args = context.eval(source)
      next hash == Digest::SHA256.hexdigest(source) unless offset

      [hash == Digest::SHA256.hexdigest(source), offset, wrapper.include?("'renderToString'"), args]
    end
    renders = [1, 2].map { |n| #{FastCallsFixtures.render("Hello", '{"n":N}', before: "", after: "").dump}.sub("N", n.to_s) }
    reflected = [hashes_source.call("(hash.apply(this, [1]))"), *renders.map { |source| hashes_source.call("(" + source + ")") },
                 hashes_source.call("(hash.apply(this, [2]))")]
    kept = %w[byCaller byFunction]
    again = kept.map { |name| context.eval("rerun('" + name + "')") }
    during = kept.map { |name| context.eval("(rerun.apply(this, [\\"" + name + "\\"]))") }
    puts JSON.generate([reflected, again, during])
  RUBY

  def reflected(switch)
    out, err, status = run_ruby(REFLECTED, env: { "MINI_RACER_RS_FAST_CALLS" => switch })

    assert_predicate status, :success?, err
    JSON.parse(out)
  end

  # Deviation behavior:fast-calls@ruby: mini_racer 0.22.1 compiles every eval,
  # so its bottom frame's CallSite#getScriptHash hashes the source and its
  # CallSite#getPosition is the source's (185 for this render), and a render's
  # wrapper, reached through `caller` or CallSite#getFunction, holds the
  # literals it passes and passes them again when called again. A fast call
  # runs a template instead, first or on a hit, which only reflection shows:
  # called again, the wrapper passes nothing outside a fast call and that
  # call's values during one.
  def test_reflection_shows_a_fast_call_runs_a_template
    args = ->(n) { ["renderToString", "Hello", { "n" => n }] }
    fast = [
      [false, [false, 260, false, args[1]], [false, 260, false, args[2]], false],
      [[nil, nil, nil]] * 2,
      [[["byCaller"], nil, nil], [["byFunction"], nil, nil]],
    ]
    normal = [[true, [true, 185, true, args[1]], [true, 185, true, args[2]], true], [args[2]] * 2, [args[2]] * 2]

    assert_equal fast, reflected(nil)
    assert_equal normal, reflected("off")
  end
end
