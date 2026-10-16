# frozen_string_literal: true

$LOAD_PATH.unshift(File.expand_path("../../lib", __dir__))

require "minitest/autorun"

# CI's GC stress job moves objects on every major collection.
GC.auto_compact = true if ENV["MINI_RACER_RS_AUTO_COMPACT"]
require "open3"
require "rbconfig"

module RubyProcess
  LIB = File.expand_path("../../lib", __dir__)

  def run_ruby(code, env: {}, load_path: [])
    Open3.capture3(env, RbConfig.ruby, "-I", LIB, *load_path.flat_map { |dir| ["-I", dir] }, "-e", code)
  end
end
