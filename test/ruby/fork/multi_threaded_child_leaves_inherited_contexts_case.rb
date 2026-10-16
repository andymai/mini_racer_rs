# frozen_string_literal: true

require_relative "prelude"

context = MiniRacer::Context.new
context.eval("1")
child do
  eval_error = raised(MiniRacer::RuntimeError) { context.eval("1") }
  raise "eval: #{eval_error.inspect}" unless eval_error&.start_with?("single-threaded runner:")
end
raise "parent" unless context.eval("2") == 2
