# frozen_string_literal: true

require "test_helper"
require "mini_racer"
require "timeout"

begin
  require "async"
rescue LoadError
  nil
end

# MiniRacerRs.timeout(milliseconds) { ... } gives every eval, eval_await, call
# and call_await the block starts the bound `timeout:` gives a context's
# scripts, and only ever tightens it. Timeout.timeout around a dispatch raises
# into whatever Ruby code is running, which a script can catch, and leaves the
# microtasks a cancelled script queued unbounded.
module TimeoutScopeFixtures
  SPIN = "while (true) {}"
  BUSY = "globalThis.busy = (ms) => { const end = Date.now() + ms; while (Date.now() < end) {} return ms };" \
         "globalThis.spin = () => { while (true) {} };" \
         "globalThis.never = () => new Promise(() => {})"

  def setup
    @context = busy_context
  end

  def teardown
    @context&.dispose
  end

  def busy_context(**options)
    MiniRacer::Context.new(**options).tap { _1.eval(BUSY) }
  end

  def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

  # Fails rather than hangs when nothing bounds the script.
  def assert_terminated(&block)
    started = now
    assert_raises(MiniRacer::ScriptTerminatedError) { Timeout.timeout(5, &block) }
    assert_operator now - started, :<, 2
  end

  def assert_unbounded(context = @context)
    assert_equal 150, context.call("busy", 150)
  end
end

class TimeoutScopeTest < Minitest::Test
  include TimeoutScopeFixtures

  def test_a_scope_ends_each_kind_of_dispatch_it_starts
    assert_terminated { MiniRacerRs.timeout(50) { @context.eval(SPIN) } }
    assert_terminated { MiniRacerRs.timeout(50) { @context.call("spin") } }
    assert_terminated { MiniRacerRs.timeout(50) { @context.eval_await("never()") } }
    assert_terminated { MiniRacerRs.timeout(50) { @context.call_await("never") } }
    assert_unbounded
  end

  def test_each_dispatch_gets_the_whole_budget_and_the_block_value_comes_back
    result = MiniRacerRs.timeout(1_000) do
      Array.new(3) { @context.call("busy", 400) }
    end

    assert_equal [400, 400, 400], result
    assert_equal :done, MiniRacerRs.timeout(50) { :done }
  end

  def test_nested_scopes_only_tighten
    assert_terminated { MiniRacerRs.timeout(60_000) { MiniRacerRs.timeout(50) { @context.eval(SPIN) } } }
    assert_terminated { MiniRacerRs.timeout(50) { MiniRacerRs.timeout(60_000) { @context.eval(SPIN) } } }
    assert_terminated do
      MiniRacerRs.timeout(50) do
        MiniRacerRs.timeout(60_000) { @context.eval("1") }
        @context.eval(SPIN)
      end
    end
    assert_equal 150, MiniRacerRs.timeout(1_000) { MiniRacerRs.timeout(0) { @context.call("busy", 150) } }
  end

  def test_the_contexts_timeout_bounds_a_looser_scope
    context = busy_context(timeout: 50)

    assert_terminated { MiniRacerRs.timeout(60_000) { context.eval(SPIN) } }
  ensure
    context&.dispose
  end

  def test_a_scope_tightens_the_contexts_timeout
    context = busy_context(timeout: 60_000)

    assert_terminated { MiniRacerRs.timeout(50) { context.eval(SPIN) } }
    assert_unbounded(context)
  ensure
    context&.dispose
  end

  def test_a_scope_closes_however_the_block_leaves
    assert_equal 7, MiniRacerRs.timeout(50) { break 7 }
    assert_equal 3, MiniRacerRs.timeout(50) { next 3 }
    assert_equal :thrown, catch(:out) { MiniRacerRs.timeout(50) { throw :out, :thrown } }
    assert_equal :early, return_from_inside
    assert_raises(ArgumentError) { MiniRacerRs.timeout(50) { raise ArgumentError } }
    assert_unbounded
  end

  def return_from_inside
    MiniRacerRs.timeout(50) { return :early }
    :late
  end

  # Milliseconds are checked as Context.new checks `timeout:`, where 0 means
  # no timeout too.
  def test_milliseconds_are_checked_as_timeout_is
    [-1, 2**31, 2**64, 1.5, "1", nil].each do |bad|
      expected = assert_raises(StandardError) { MiniRacer::Context.new(timeout: bad) }
      error = assert_raises(StandardError) { MiniRacerRs.timeout(bad) { flunk "the block ran" } }

      assert_equal [expected.class, expected.message], [error.class, error.message], bad.inspect
    end
    assert_equal 150, MiniRacerRs.timeout(0) { @context.call("busy", 150) }
    assert_equal 150, MiniRacerRs.timeout((2**31) - 1) { @context.call("busy", 150) }
  end

  def test_a_scope_needs_a_block
    error = assert_raises(LocalJumpError) { MiniRacerRs.timeout(50) }

    assert_equal "no block given (yield)", error.message
    assert_raises(ArgumentError) { MiniRacerRs.timeout { 1 } }
    assert_unbounded
  end
end

class TimeoutScopeCallbackTest < Minitest::Test
  include TimeoutScopeFixtures
  include RubyProcess

  # The watchdog terminates the script, so nothing raises into the callback
  # for it to rescue, and JavaScript cannot catch the termination.
  def test_a_script_that_catches_errors_from_a_callback_still_ends
    nap = proc do
      sleep 0.2
      1
    rescue Exception # rubocop:disable Lint/RescueException
      2
    end
    @context.attach("nap", nap)

    assert_terminated { MiniRacerRs.timeout(50) { @context.eval("try { nap() } catch (e) {} #{SPIN}") } }
  end

  def test_a_callback_that_rescues_its_nested_termination_does_not_save_the_script
    rescued = 0
    nest = proc do
      @context.eval(SPIN)
    rescue MiniRacer::ScriptTerminatedError
      rescued += 1
    end
    @context.attach("nest", nest)

    assert_terminated { MiniRacerRs.timeout(50) { @context.eval("try { nest() } catch (e) {} #{SPIN}") } }
    assert_equal 1, rescued
  end

  # A pump is never timed, so a script a callback runs inside one gets the
  # scope's timeout of its own, as it gets the context's.
  def test_a_script_nested_in_a_pump_gets_the_scopes_timeout
    outcome = nil
    nest = proc do
      @context.eval(SPIN)
    rescue MiniRacer::ScriptTerminatedError
      outcome = :terminated
    end
    @context.attach("nest", nest)
    @context.eval("Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1).value.then(() => nest())")
    sleep 0.05

    assert Timeout.timeout(5) { MiniRacerRs.timeout(50) { @context.pump_message_loop } }
    assert_equal :terminated, outcome
  end

  def test_a_scope_bounds_the_microtasks_a_terminated_script_left
    assert_terminated do
      MiniRacerRs.timeout(100) { @context.eval("Promise.resolve().then(() => { #{SPIN} }); #{SPIN}") }
    end
    assert_equal 2, @context.eval("1 + 1")
  end

  def test_execjs_compiles_and_calls_under_a_scope
    execjs = File.expand_path("../../vendor/execjs/lib", __dir__)
    raise "vendor/execjs is empty; run `git submodule update --init`" unless File.file?(File.join(execjs, "execjs.rb"))

    out, err, status = run_ruby(<<~RUBY, env: { "EXECJS_RUNTIME" => "MiniRacer" }, load_path: [execjs])
      require "execjs"
      require "mini_racer_rs"
      begin
        MiniRacerRs.timeout(50) { ExecJS.compile("#{SPIN}") }
      rescue MiniRacer::ScriptTerminatedError => e
        puts "compile: \#{e.message}"
      end
      context = ExecJS.compile("function spin() { #{SPIN} } function add(a, b) { return a + b }")
      begin
        MiniRacerRs.timeout(50) { context.call("spin") }
      rescue MiniRacer::ScriptTerminatedError => e
        puts "call: \#{e.message}"
      end
      puts MiniRacerRs.timeout(50) { context.call("add", 1, 2) }
    RUBY

    assert_predicate status, :success?, err
    assert_equal "compile: terminated\ncall: terminated\n3\n", out
  end
end

# A scope opened inside a callback times the script the callback runs on the
# context whenever nothing around that script ends sooner: inside an untimed
# eval, inside a looser scope or `timeout:`, and inside a pump. The script
# around it carries on once the nested one has ended, still under its own
# bound.
class TimeoutScopeNestedTest < Minitest::Test
  include TimeoutScopeFixtures

  def setup
    super
    @outcomes = []
    attach_nest(@context)
  end

  def attach_nest(context)
    [["nest", 50], ["loose", 60_000]].each do |name, milliseconds|
      nest = proc do
        MiniRacerRs.timeout(milliseconds) { context.eval(SPIN) }
      rescue MiniRacer::ScriptTerminatedError
        @outcomes << :terminated
        1
      end
      context.attach(name, nest)
    end
  end

  # Fails rather than hangs when nothing bounds the nested script.
  def assert_nested_ended(&)
    started = now
    result = begin
      Timeout.timeout(5, &)
    rescue Timeout::Error
      :unbounded
    end

    assert_equal 2, result
    assert_operator now - started, :<, 2
    assert_equal [:terminated], @outcomes
  end

  def test_inside_an_untimed_eval
    assert_nested_ended { @context.eval("nest() + 1") }
  end

  def test_inside_a_looser_scope
    assert_nested_ended { MiniRacerRs.timeout(60_000) { @context.eval("nest() + 1") } }
  end

  def test_inside_a_looser_context_timeout
    context = busy_context(timeout: 60_000)
    attach_nest(context)

    assert_nested_ended { context.eval("nest() + 1") }
  ensure
    context&.dispose
  end

  def test_inside_a_pump
    @context.eval("Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1).value" \
                  ".then(() => { globalThis.result = nest() + 1 })")
    sleep 0.05

    assert_nested_ended do
      @context.pump_message_loop
      @context.eval("result")
    end
  end

  def test_the_outer_bound_still_ends_the_script_around_it
    assert_terminated { MiniRacerRs.timeout(300) { @context.eval("nest(); #{SPIN}") } }
    assert_equal [:terminated], @outcomes

    context = busy_context(timeout: 300)
    attach_nest(context)

    assert_terminated { context.eval("nest(); #{SPIN}") }
  ensure
    context&.dispose
  end

  def test_a_looser_nested_scope_leaves_the_outer_bound
    assert_terminated { MiniRacerRs.timeout(50) { @context.eval("loose() + 1") } }
    assert_equal [:terminated], @outcomes
  end
end

# A block left by break or throw resumes through the jump Ruby keeps pending,
# so restoring the fiber's storage must neither clear it nor drop an
# exception or a trap an interrupt brings meanwhile. Each case loops for a few
# seconds under a stream of Thread#raise or signal traps, in a child that
# kills itself if it outlives its bound.
class TimeoutScopeInterruptTest < Minitest::Test
  include RubyProcess

  SECONDS = 3

  PRELUDE = <<~RUBY.freeze
    require "mini_racer"
    Thread.new { sleep #{SECONDS * 10}; Process.kill(:KILL, Process.pid) }
    def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    deadline = now + #{SECONDS}
  RUBY

  CASES = {
    "break" => "MiniRacerRs.timeout(50) { break }",
    "throw" => "catch(:out) { MiniRacerRs.timeout(50) { throw :out } }",
  }.freeze

  def raced(leave)
    <<~RUBY
      #{PRELUDE}
      E = Class.new(StandardError)
      target = Thread.current
      Thread.new { loop { target.raise(E); sleep 0.0005 } }
      raised = 0
      while now < deadline
        begin
          loop { #{leave} }
        rescue E
          raised += 1
        end
      end
      puts raised.positive?
    RUBY
  end

  TRAPPED = <<~RUBY.freeze
    #{PRELUDE}
    handled = 0
    trap("USR1") do
      raise "inside the trap"
    rescue RuntimeError
      handled += 1
    end
    Thread.new { loop { Process.kill(:USR1, Process.pid); sleep 0.0005 } }
    MiniRacerRs.timeout(50) { break } while now < deadline
    puts handled.positive?
  RUBY

  def run_cases(codes)
    codes.map { |code| Thread.new { run_ruby(code) } }.map(&:value).each do |out, err, status|
      assert_predicate status, :success?, err.lines.first(3).join
      assert_equal "true\n", out
    end
  end

  def test_interrupts_and_traps_while_a_block_breaks_or_throws_out
    run_cases([*CASES.values.map { raced(_1) }, TRAPPED])
  end
end

class TimeoutScopeFiberTest < Minitest::Test
  include TimeoutScopeFixtures

  def test_a_scope_reaches_fibers_and_threads_started_inside_it_while_it_is_open
    child = nil
    assert_terminated do
      MiniRacerRs.timeout(50) do
        child = Fiber.new { @context.call("busy", 150) }
        Fiber.new { @context.eval(SPIN) }.resume
      end
    end
    assert_terminated do
      MiniRacerRs.timeout(50) do
        Thread.new do
          Thread.current.report_on_exception = false
          @context.eval(SPIN)
        end.value
      end
    end
    assert_equal 150, child.resume
  end

  def test_a_scope_on_one_fiber_leaves_another_alone
    other = busy_context
    holder = Fiber.new do
      MiniRacerRs.timeout(50) do
        Fiber.yield
        other.eval(SPIN)
      end
    end
    holder.resume

    assert_unbounded
    assert_equal 150, Fiber.new { @context.call("busy", 150) }.resume
    assert_equal 150, Thread.new { @context.call("busy", 150) }.value
    assert_terminated { holder.resume }
  ensure
    other&.dispose
  end

  # The scope ends the script, not the fiber's wait: other fibers keep running
  # meanwhile, and an Async task started inside the scope inherits it.
  def test_a_scope_bounds_dispatches_waited_on_under_a_scheduler
    skip "needs CRuby and the async gem" unless RUBY_ENGINE == "ruby" && defined?(Async)

    ticks = 0
    errors = []
    Sync do |task|
      ticker = task.async do
        loop do
          sleep 0.005
          ticks += 1
        end
      end
      MiniRacerRs.timeout(100) do
        @context.eval(SPIN)
      rescue MiniRacer::ScriptTerminatedError => e
        errors << e
      end
      MiniRacerRs.timeout(100) do
        task.async { @context.eval(SPIN) }.wait
      rescue MiniRacer::ScriptTerminatedError => e
        errors << e
      end
      ticker.stop
    end

    assert_equal 2, errors.size
    assert_operator ticks, :>=, 5
    assert_unbounded
  end
end
