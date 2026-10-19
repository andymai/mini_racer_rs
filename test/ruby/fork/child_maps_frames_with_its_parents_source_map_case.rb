# frozen_string_literal: true

require_relative "prelude"

# A source map registered before a fork is the child's too: a context the
# child takes over keeps the scripts it matched, and a new context matches the
# bundle again. A map the child registers maps there and never in the parent.
# mini_racer has no source maps; its frames give the evaluated source's
# positions.
MiniRacer::Platform.set_flags!(:single_threaded)

# Each bundle passes the registry's 1 KiB minimum on its last line.
def bundle(name)
  "function #{name}() { throw new Error('boom'); }\n" \
    "function render#{name}() { return #{name}(); } /*#{"pad " * 256}*/\n"
end

def map(root)
  %({"version":3,"sourceRoot":"#{root}","sources":["./src/Header.jsx"],"mappings":"AAAA;AACA"})
end

def frames(context, call)
  context.eval(call)
  raise "no error"
rescue MiniRacer::RuntimeError => e
  e.backtrace.grep(/\AJavaScript /).first(2)
end

def fresh(name)
  context = MiniRacer::Context.new
  context.eval(bundle(name))
  frames(context, "render#{name}()")
ensure
  context&.dispose
end

def mapped(name, root)
  ["JavaScript at #{name} (#{root}./src/Header.jsx:1:1)", "JavaScript at render#{name} (#{root}./src/Header.jsx:2:1)"]
end

PARENT = mapped("Ue", "webpack://parent/")

MiniRacerRs.register_source_map(bundle("Ue"), map("webpack://parent/"))
inherited = MiniRacer::Context.new
inherited.eval(bundle("Ue"))
raise "parent: #{frames(inherited, "renderUe()")}" unless frames(inherited, "renderUe()") == PARENT

child do
  raise "taken over: #{frames(inherited, "renderUe()")}" unless frames(inherited, "renderUe()") == PARENT
  raise "new context: #{fresh("Ue")}" unless fresh("Ue") == PARENT

  MiniRacerRs.register_source_map(bundle("Other"), map("webpack://child/"))
  raise "the child's map: #{fresh("Other")}" unless fresh("Other") == mapped("Other", "webpack://child/")
end
raise "parent after: #{frames(inherited, "renderUe()")}" unless frames(inherited, "renderUe()") == PARENT
raise "the child's map in the parent: #{fresh("Other")}" unless fresh("Other").grep(/webpack:/).empty?
