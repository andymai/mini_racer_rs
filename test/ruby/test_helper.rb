# frozen_string_literal: true

$LOAD_PATH.unshift(File.expand_path("../../lib", __dir__))

require "minitest/autorun"
require "open3"
require "rbconfig"

module RubyProcess
  LIB = File.expand_path("../../lib", __dir__)

  def run_ruby(code)
    Open3.capture3(RbConfig.ruby, "-I", LIB, "-e", code)
  end
end
