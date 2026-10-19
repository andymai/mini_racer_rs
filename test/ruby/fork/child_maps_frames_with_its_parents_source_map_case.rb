# frozen_string_literal: true

require_relative "prelude"

# A source map registered before a fork is the child's too: a context the
# child takes over keeps the scripts it matched, a new context matches the
# bundle again, and the child registers maps of its own past its parent's.
# mini_racer has no source maps; its frames give the evaluated source's
# positions.
MiniRacer::Platform.set_flags!(:single_threaded)

BUNDLE = "function Ue() { throw new Error('boom'); }\nfunction render() { return Ue(); }\n"
MAP = '{"version":3,"sourceRoot":"webpack://app/","sources":["./src/Header.jsx"],"mappings":"AAAA;AACA"}'
MAPPED = [
  "JavaScript at Ue (webpack://app/./src/Header.jsx:1:1)",
  "JavaScript at render (webpack://app/./src/Header.jsx:2:1)",
].freeze

def frames(context)
  context.eval("render()")
  raise "no error"
rescue MiniRacer::RuntimeError => e
  e.backtrace.grep(/\AJavaScript /).first(2)
end

def fresh
  context = MiniRacer::Context.new
  context.eval(BUNDLE)
  frames(context)
ensure
  context&.dispose
end

MiniRacerRs.register_source_map(BUNDLE, MAP)
inherited = MiniRacer::Context.new
inherited.eval(BUNDLE)
raise "parent: #{frames(inherited)}" unless frames(inherited) == MAPPED

child do
  raise "taken over: #{frames(inherited)}" unless frames(inherited) == MAPPED
  raise "new context: #{fresh}" unless fresh == MAPPED

  MiniRacerRs.register_source_map("var other = 1;\n", MAP)
end
raise "parent after: #{frames(inherited)}" unless frames(inherited) == MAPPED
