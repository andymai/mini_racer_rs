# frozen_string_literal: true

require "test_helper"
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

  # react-rails 3.3.1's console_reset.js and console_replay.js, which
  # BundleRenderer puts around every render when it replays the console.
  RESET = "if (typeof console !== \"undefined\" && console.history) {\n  console.history = [];\n}\n"
  REPLAY = <<~JS
    (function (history) {
      if (history && history.length > 0) {
        result += '\\n<scr'+'ipt class="react-rails-console-replay">';
        history.forEach(function (msg) {
          result += '\\nconsole.' + msg.level + '.apply(console, ' + JSON.stringify(msg.arguments) + ');';
        });
        result += '\\n</scr'+'ipt>';
      }
    })(console.history);
  JS

  THROWN = ["Error: boom {\"k\":1}", ["inner (<eval>:2:27)", "outer (<eval>:3:28)", "<eval>:1:8"]].freeze
  RENDER_THROWN = ["TypeError: bad 3", ["Object.serverRender (<eval>:16:34)", "<eval>:6:45", "<eval>:18:13"]].freeze

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

  def render(component, props, replay: true)
    main = "this.ReactRailsUJS.serverRender('renderToString', '#{component}', #{props})"
    compose_js(replay ? RESET : "", main, replay ? REPLAY : "")
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
    replayed = "<renderToString:Logs>{\"n\":3}\n<script class=\"react-rails-console-replay\">\n" \
               "console.log.apply(console, [\"hi\",3]);\n</script>"

    assert_equal "<renderToString:Hello>{\"name\":\"W\u00e9\"}",
                 @context.eval(render("Hello", { "name" => "W\u00e9" }.to_json))
    assert_equal replayed, @context.eval(render("Logs", '{"n":3}'))
    assert_equal "<renderToString:Logs>{\"n\":3}", @context.eval(render("Logs", '{"n":3}', replay: false))
  end

  def test_render_errors_carry_mini_racers_frames_before_and_after_the_props
    circular = [
      "JavaScript --> starting at object with constructor 'Object'",
      "JavaScript --- property 'o' closes the circle",
      "JSON.stringify ((execjs))", "<eval>:11:70", "Array.forEach ((execjs))", "<eval>:10:13", "<eval>:15:3",
      "<eval>:18:13",
    ]

    cyclic = failure { @context.eval(render("Cyclic", "{}")) }

    assert_equal(RENDER_THROWN, failure { @context.eval(render("Throws", '{"n":3}')) })
    assert_equal ["TypeError: Converting circular structure to JSON", circular], cyclic
  end

  def test_props_that_are_not_json_on_one_line_render_as_mini_racer_renders_them
    pretty = ["Object.serverRender (<eval>:16:34)", "<eval>:6:45", "<eval>:20:13"]

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

  # Each source compiled to a script V8 kept until a full collection: 40,000
  # of these calls grew the heap by 30 MB, and as many renders by 97 MB.
  def test_calls_and_renders_keep_no_compiled_script_each
    context = MiniRacer::Context.new
    context.eval(SOURCE)
    calls = heap_growth(context) { |i| context.eval("(add.apply(this, [#{i},2]))") }
    renders = heap_growth(context) { |i| context.eval("(#{render("Hello", { "n" => i }.to_json)})") }

    assert_operator calls, :<, 8_000_000, "the heap grew #{calls} bytes over 40,000 calls"
    assert_operator renders, :<, 8_000_000, "the heap grew #{renders} bytes over 40,000 renders"
  ensure
    context&.dispose
  end

  def heap_growth(context, &)
    context.low_memory_notification
    before = context.heap_stats[:used_heap_size]
    40_000.times(&)
    context.heap_stats[:used_heap_size] - before
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
                ["<eval>:6:31", "<eval>:18:13"],]

    assert_predicate status, :success?, err
    assert_equal [true, "Error: s\n    at stack (<eval>:4:27)\n    at <eval>:1:8", rendered], JSON.parse(out)
  end

  # mini_racer's verbose TryCatch prints the offset in the script where an
  # exception happened, past the props here.
  def test_verbose_exceptions_report_the_offset_mini_racer_reports
    replay = "(function (history) {\n  JSON.stringify(history[0].arguments);\n})(console.history);\n"
    main = "this.ReactRailsUJS.serverRender('renderToString', 'Cyclic', {\"n\":1})"
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      context = MiniRacer::Context.new(verbose_exceptions: true)
      context.eval(#{SOURCE.dump})
      begin
        context.eval(#{"(#{compose_js(RESET, main, replay)})".dump})
      rescue MiniRacer::RuntimeError
        nil
      end
    RUBY

    assert_predicate status, :success?, err
    assert_equal "<eval>:258: Uncaught TypeError: Converting circular structure to JSON\n    " \
                 "--> starting at object with constructor 'Object'\n    " \
                 "--- property 'o' closes the circle\n",
                 out
  end
end
