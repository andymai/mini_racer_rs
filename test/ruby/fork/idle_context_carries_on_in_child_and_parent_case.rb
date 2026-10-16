# frozen_string_literal: true

require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

context = MiniRacer::Context.new
context.eval("var count = 1")
child do
  raise "child eval" unless context.eval("++count") == 2

  fresh = MiniRacer::Context.new
  raise "new context" unless fresh.eval("40 + 2") == 42

  fresh.dispose
  context.dispose
end
raise "parent eval" unless context.eval("++count") == 2
