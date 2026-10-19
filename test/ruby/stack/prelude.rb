# frozen_string_literal: true

# Loads this gem for a stack case and gives it `near_the_limit`, which runs a
# block in a new fiber or thread a few levels short of where Ruby raises
# SystemStackError, so the block starts with little more than the margin Ruby
# keeps for a C function; `check_the_dive`, which fails the case if it does
# not; `nested`, a script whose result nests a shape 95 deep around a bottom
# value; `outcome`, a block's value or the exception it raised;
# `check_outcome`, which fails the case unless an outcome is one it allows or
# Ruby's own SystemStackError; `check_decoded`, which fails a release build's
# case unless some gap gave the value; and `usable`, which fails the case
# unless the context still evaluates and disposes.

$LOAD_PATH.unshift File.expand_path("../../../lib", __dir__)
require "mini_racer"

SHAPES = {
  "objects" => "v = {a: v}",
  "arrays" => "v = [v]",
  "maps" => "v = new Map([['a', v]])",
  "sets" => "v = new Set([v])",
  "errors" => "{ const e = new Error('m'); e.cause = v; v = e }",
  "sparse arrays" => "{ const a = [v]; a[1000] = 1; v = a }",
  "objects with Latin-1 text" => "v = {['\\u00e9' + i]: v, s: '\\u00e9'.repeat(100)}",
}.freeze

# Levels of C method recursion short of a stack's limit, a KiB each on Linux
# x86_64: from where Ruby's own checks refuse a call to well past where a
# release build there first decodes a shape 95 deep (24 to 44 levels).
GAPS = [0, 2, 4, 8, 16, 24, 32, 40, 48, 64, 80].freeze

def nested(step, levels: 95, bottom: "1")
  "(() => { let v = #{bottom}; for (let i = 0; i < #{levels}; i++) #{step}; return v })()"
end

# Array#map is a C method, so each level takes machine stack, which Ruby
# checks at every call, as well as VM stack.
def deepest(levels = 0)
  [0].map { deepest(levels + 1) }.first
rescue SystemStackError
  levels
end

def dive(levels, &)
  levels <= 0 ? yield : [0].map { dive(levels - 1, &) }.first
end

def on(where, &)
  where == :fiber ? Fiber.new(&).resume : Thread.new(&).value
end

def near_the_limit(where, gap, &)
  levels = on(where) { deepest }
  on(where) { dive(levels - gap, &) }
end

# Fails the case if diving left more room than it should, which would leave
# the case testing nothing.
def check_the_dive
  %i[fiber thread].each do |where|
    left = near_the_limit(where, 0) { deepest }
    raise "a #{where} #{left} levels short of its limit after diving to it" if left > 1
  end
end

def outcome
  yield
rescue Exception => e # rubocop:disable Lint/RescueException
  e
end

def check_outcome(answer, label, *allowed)
  return if allowed.include?(answer)
  return if answer.instance_of?(SystemStackError) && answer.message == "stack level too deep"

  raise "#{label}: #{answer.inspect[0, 300]}"
end

# CI builds the release profile. A debug build's frames are many times larger,
# so it refuses at every gap.
def check_decoded(answers, label, whole)
  return unless ENV["CI"]
  return if answers.include?(whole)

  raise "#{label}: refused at every gap up to #{GAPS.last} levels short of the limit"
end

def usable(context)
  answer = outcome { context.eval("1 + 1") }
  raise "the context no longer evaluates: #{answer.inspect}" unless answer == 2

  disposed = outcome { context.dispose }
  raise "the context no longer disposes: #{disposed.inspect}" if disposed.is_a?(Exception)
end
