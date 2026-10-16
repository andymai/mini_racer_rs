# frozen_string_literal: true

# As in mini_racer, a context disposed before the fork stays disposed in the
# child: calls raise ContextDisposedError and dispose returns.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)
context = MiniRacer::Context.new
context.eval("1")
context.dispose
child do
  error = raised(MiniRacer::ContextDisposedError) { context.eval("1") }
  raise "eval: #{error.inspect}" unless error

  context.dispose
  error = raised(MiniRacer::ContextDisposedError) { context.attach("f", proc { 1 }) }
  raise "attach: #{error.inspect}" unless error
end
