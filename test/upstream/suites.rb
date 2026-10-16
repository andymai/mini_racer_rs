# frozen_string_literal: true

require_relative "ledger"

# The upstream suites the runner can run, each against its own ledger, so the
# runner and `rake ledger:merge` agree on what a suite name means.
module UpstreamSuites
  ALL = {
    "mini_racer" => {
      root: "vendor/mini_racer", load_path: ["test"], env: {},
      files: -> { (Dir["test/*_test.rb"] + ["test/test_object_identity_fix.rb"]).sort },
    },
    "execjs" => {
      root: "vendor/execjs", load_path: %w[lib test], env: { "EXECJS_RUNTIME" => "MiniRacer" },
      files: -> { ["test/test_execjs.rb"] },
    },
  }.freeze

  module_function

  def fetch(name)
    ALL.fetch(name) { raise ArgumentError, "unknown suite #{name.inspect}; one of #{ALL.keys.join(", ")}" }
  end

  def root(name)
    File.join(UpstreamLedger::ROOT, fetch(name).fetch(:root))
  end

  def ledger(name)
    fetch(name)
    File.join(__dir__, "#{name}.status.toml")
  end
end
