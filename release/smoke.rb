# frozen_string_literal: true

# Run against an installed gem from outside the repository: loads it as an
# application does and crosses into the engine each way a release must.
#   EXPECTED_PLATFORM=<gem platform> ruby release/smoke.rb
require "mini_racer"

spec = Gem.loaded_specs.fetch("mini_racer_rs")
expected = ENV.fetch("EXPECTED_PLATFORM")
abort "loaded #{spec.full_name} (#{spec.platform}), not the #{expected} gem" unless spec.platform.to_s == expected

def check(name)
  result = yield
  abort "#{name}: got #{result.inspect}" unless result
  puts "ok #{name}"
end

context = MiniRacer::Context.new
check("eval") { context.eval("1 + 1") == 2 }
check("call") { context.call("JSON.stringify", { "a" => [1, 2] }) == '{"a":[1,2]}' }
context.attach("ruby.add", ->(a, b) { a + b })
check("attach") { context.eval("ruby.add(2, 3)") == 5 }

if RUBY_ENGINE == "ruby"
  check("icu") { context.eval("new Date('April 28 2021').toLocaleDateString('es-MX')") == "28/4/2021" }
  timed = MiniRacer::Context.new(timeout: 50)
  check("timeout") do
    timed.eval("while (true) {}")
  rescue MiniRacer::ScriptTerminatedError
    true
  end
  snapshot = MiniRacer::Snapshot.new("var answer = 42")
  check("snapshot") { MiniRacer::Context.new(snapshot: snapshot).eval("answer") == 42 }
  context.dispose
  puts "#{spec.full_name} on Ruby #{RUBY_VERSION}, V8 #{MiniRacerRs::V8_VERSION}"
else
  puts "#{spec.full_name} on #{RUBY_DESCRIPTION}"
end
