# frozen_string_literal: true

# Checks the calls the Ruby tests make into MiniRacer and MiniRacerRs against
# sig/. rbs/test/setup hooks a class only when a Ruby class body for it ends,
# which the extension's classes never have, so the hooks go in here once it
# has loaded.
#
# Only calls that return are checked, with their blocks: the tests pass wrong
# types on purpose to assert the TypeError the method raises, and RBS types no
# exceptions. Mismatches are collected rather than raised, so every test runs
# as it would without the checker, and fail the run at exit.

require "rbs"
require "rbs/test"
require "mini_racer"

module RuntimeCheck
  SIG = File.expand_path("../../sig", __dir__)
  ROOTS = %w[::MiniRacer ::MiniRacerRs].freeze

  @mismatches = []
  @checked = 0
  @pid = Process.pid

  class << self
    attr_reader :mismatches

    def checked = @checked += 1

    def install
      RBS.logger_level = ENV.fetch("RBS_TEST_LOGLEVEL", "warn")
      loader = RBS::EnvironmentLoader.new
      loader.add(path: Pathname(SIG))
      env = RBS::Environment.from_loader(loader).resolve_type_names
      tester = RBS::Test::Tester.new(env:)
      targets = env.class_decls.keys.select do |name|
        ROOTS.any? { name.to_s == _1 || name.to_s.start_with?("#{_1}::") }
      end
      modules = targets.to_h { [_1, Object.const_get(_1.to_s)] }
      unsigned = modules.flat_map { |name, mod| unsigned(tester.builder, name, mod) }
      raise "methods without a signature in sig/: #{unsigned.join(", ")}" unless unsigned.empty?

      modules.each_value { tester.install!(_1, sample_size: 100, unchecked_classes: []) }
    end

    # A test's forked child exits through here too, with its parent's counts.
    def report
      return unless Process.pid == @pid

      if mismatches.empty?
        warn "rbs: #{@checked} returning calls matched sig/"
        return
      end
      warn "rbs: calls that did not match sig/:"
      mismatches.tally.each { |mismatch, count| warn "  #{mismatch} (#{count}x)" }
      exit 1
    end

    private

    def unsigned(builder, name, mod)
      instance = builder.build_instance(name).methods
      singleton = builder.build_singleton(name).methods
      missing = mod.public_instance_methods(false).reject { instance.key?(_1) }.map { "#{name}##{_1}" }
      missing + mod.singleton_methods(false).reject { singleton.key?(_1) }.map { "#{name}.#{_1}" }
    end
  end

  module ReturnedCalls
    def call(receiver, trace)
      return if trace.method_call.exception?

      RuntimeCheck.checked
      super
    rescue RBS::Test::Tester::TypeError => e
      RuntimeCheck.mismatches.concat(e.errors.map { RBS::Test::Errors.to_string(_1) })
    end
  end
end

RBS::Test::Tester::MethodCallTester.prepend(RuntimeCheck::ReturnedCalls)
RuntimeCheck.install
at_exit { RuntimeCheck.report }
