# frozen_string_literal: true

# A server rendering bundle kept outside the repository: the file named by
# MINI_RACER_RS_BENCH_SSR_BUNDLE, loaded as react-rails loads one, after the
# globals and stubs its renderer defines and the file named by
# MINI_RACER_RS_BENCH_SSR_PRELUDE. MINI_RACER_RS_BENCH_SSR_CALL is the
# JavaScript expression that renders. What the benchmark prints and writes
# leaves out the bundle's path and name and every error message it raises.
module BenchExternal
  BUNDLE = ENV.fetch("MINI_RACER_RS_BENCH_SSR_BUNDLE", "")
  PRELUDE = ENV.fetch("MINI_RACER_RS_BENCH_SSR_PRELUDE", "")
  CALL = ENV.fetch("MINI_RACER_RS_BENCH_SSR_CALL", "")
  # react-rails' console keeps every message to replay in the page; this one
  # drops them, so thousands of renders do not pile them up.
  RENDERER = <<~JS
    var global = global || this;
    var self = self || this;
    var console = { log() {}, info() {}, warn() {}, error() {} };
    function setTimeout() {}
    function clearTimeout() {}
  JS

  module_function

  def enabled? = !BUNDLE.empty?

  def call? = !CALL.empty?

  def source
    [RENDERER, *(PRELUDE.empty? ? [] : [File.read(PRELUDE)]), File.read(BUNDLE)].join("\n")
  end
end
