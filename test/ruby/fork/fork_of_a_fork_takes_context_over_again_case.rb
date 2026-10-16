# frozen_string_literal: true

require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

context = MiniRacer::Context.new
context.eval("var depth = 0")
child do
  raise "child" unless context.eval("++depth") == 1

  child { raise "grandchild" unless context.eval("++depth") == 2 }
  raise "child after" unless context.eval("++depth") == 2
end
raise "parent" unless context.eval("depth").zero?
