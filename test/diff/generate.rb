# frozen_string_literal: true

require "json"

# Seeded cases for the differential run. A JS case is a value tree rendered to
# source and evaluated; a Ruby case is a list of canonical values passed through
# Context#call and returned. Trees, not source, are kept so the shrinker can cut
# them down.
module DiffGenerate
  JS_LEAVES = [
    "undefined", "null", "true", "false", "0", "-0", "1", "-1", "2147483647", "-2147483648", "2147483648",
    "4294967295", "4294967296", "9007199254740991", "9007199254740992", "1.5", "-1.5", "0.1", "1e300", "5e-324",
    "NaN", "Infinity", "-Infinity", "0n", "10n", "-10n", "2n ** 64n", "-(2n ** 200n)", "''", "'a'", "'\\u00e9'",
    "'\\u65e5\\u672c'", "'\\u{1F600}'", "'\\ud800'", "'a\\u0000b'", "'x'.repeat(300)", "Symbol('s')", "Symbol()",
    "new Date(0)", "new Date(1e12 + 0.5)", "/a+/g", "(() => 1)", "new Uint8Array([1, 2, 255])", "new ArrayBuffer(4)",
    "new Error('boom')", "new TypeError('t')", "[1, , 3]", "new WeakMap()", "Object.create(null)", "new Number(1)",
  ].freeze
  KEYS = ["a", "b", "z", "1", "10", "4294967295", "-1", "1.5", "", "é", "constructor"].freeze
  RUBY_LEAVES = [
    ["nil"], ["bool", true], ["bool", false], ["int", "0"], ["int", "-1"], ["int", "2147483648"],
    ["int", (2**62).to_s], ["int", (2**64).to_s], ["int", (-(2**70)).to_s], ["float", [1.5].pack("G").unpack1("H*")],
    ["float", [-0.0].pack("G").unpack1("H*")], ["float", "nan"], ["float", [Float::INFINITY].pack("G").unpack1("H*")],
    ["string", "UTF-8", "text", ""], ["string", "UTF-8", "text", "a"], ["string", "UTF-8", "text", "é\u{1F600}"],
    ["string", "ISO-8859-1", "hex", "e9"], ["string", "ASCII-8BIT", "hex", "ff00"],
    ["string", "UTF-16LE", "hex", "6800"],
    ["symbol", "sym"], ["time", "0", 0], ["time", "1600000000", 123_456_789], ["binary", "0102ff"],
  ].freeze

  module_function

  def cases(seed:, javascript:, ruby:)
    random = Random.new(seed)
    js_cases = Array.new(javascript) { |n| { "id" => "js-#{seed}-#{n}", "kind" => "js", "tree" => js_tree(random, 3) } }
    ruby_cases = Array.new(ruby) do |n|
      { "id" => "rb-#{seed}-#{n}", "kind" => "ruby",
        "args" => Array.new(random.rand(1..3)) { canonical(ruby_tree(random, 3)) }, }
    end
    js_cases + ruby_cases
  end

  def js_tree(random, depth)
    return ["lit", JS_LEAVES.sample(random: random)] if depth.zero? || random.rand < 0.45

    children = -> { Array.new(random.rand(0..3)) { js_tree(random, depth - 1) } }
    case random.rand(5)
    when 0 then ["array", children.call]
    when 1 then ["object", children.call.map { |child| [KEYS.sample(random: random), child] }]
    when 2 then ["map", children.call.map { |child| [js_tree(random, 0), child] }]
    when 3 then ["set", children.call]
    else ["shared", js_tree(random, depth - 1)]
    end
  end

  HASH_KEYS = [%w[string UTF-8 text k], %w[symbol s], %w[int 7],
               %w[string UTF-8 text é],].freeze

  def ruby_tree(random, depth)
    return RUBY_LEAVES.sample(random: random) if depth.zero? || random.rand < 0.5

    children = Array.new(random.rand(0..3)) { ruby_tree(random, depth - 1) }
    case random.rand(3)
    when 0 then ["array", children]
    when 1 then ["hash", HASH_KEYS.sample(children.size, random: random).zip(children)]
    else ["shared", ["array", children]]
    end
  end

  # A ["shared", container] node becomes an array holding that container twice,
  # the second time as a ref numbered the way DiffCanonical numbers containers.
  def canonical(node, next_id = [0])
    case node.first
    when "array"
      next_id[0] += 1
      ["array", node[1].map { |child| canonical(child, next_id) }]
    when "hash"
      next_id[0] += 1
      ["hash", node[1].map { |key, value| [canonical(key, next_id), canonical(value, next_id)] }]
    when "shared"
      next_id[0] += 1
      inner = next_id[0]
      ["array", [canonical(node[1], next_id), ["ref", inner]]]
    else node
    end
  end

  def render(tree)
    tag, *rest = tree
    case tag
    when "lit" then rest.fetch(0)
    when "array" then "[#{rest.fetch(0).map { |child| render(child) }.join(", ")}]"
    when "object" then "({#{rest.fetch(0).map { |key, child| "#{JSON.generate(key)}: #{render(child)}" }.join(", ")}})"
    when "map" then "new Map([#{rest.fetch(0).map { |key, child| "[#{render(key)}, #{render(child)}]" }.join(", ")}])"
    when "set" then "new Set([#{rest.fetch(0).map { |child| render(child) }.join(", ")}])"
    when "shared" then "(() => { const s = #{render(rest.fetch(0))}; return [s, s]; })()"
    else raise ArgumentError, "cannot render #{tag.inspect}"
    end
  end

  ECHO = "globalThis.__diff_echo = (...args) => args"

  def steps(kase)
    case kase.fetch("kind")
    when "js" then [{ "op" => "eval", "source" => render(kase.fetch("tree")) }]
    when "ruby" then [{ "op" => "eval", "source" => ECHO },
                      { "op" => "call", "path" => "__diff_echo", "args" => kase.fetch("args") },]
    else kase.fetch("steps")
    end
  end
end
