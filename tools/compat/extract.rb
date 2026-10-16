# frozen_string_literal: true

# Surface that runtime reflection cannot see in upstream's C extension: keyword
# arguments parsed by hand from a kwargs hash, and the standard libraries its
# Ruby files require. The output is reviewed in the diff of compat/surface.json.
module CompatExtract
  UPSTREAM = File.expand_path("../../vendor/mini_racer", __dir__)
  DEFINE_CLASS = /(?:(\w+)\s*=\s*)+rb_define_(?:class|module)(?:_under)?\((?:\w+,\s*)?"(\w+)"/
  DEFINE_METHOD = /rb_define_method\((\w+),\s*"([^"]+)",\s*(\w+),/
  FUNCTION = /^(?:static\s+)?VALUE\s*\n?\s*(\w+)\(([^)]*)\)\s*\{\n(.*?)^\}/m
  KEYWORD_PATTERNS = [
    /!strcmp\(s, "(\w+)"\)/,
    /rb_hash_aref\(kwargs, rb_id2sym\(rb_intern\("(\w+)"\)\)\)/,
  ].freeze

  module_function

  def required_features
    features = Dir[File.join(UPSTREAM, "lib", "**", "*.rb")].flat_map do |file|
      File.read(file).scan(/^\s*require\s+["']([^"']+)["']/).flatten
    end
    features.reject { |feature| feature.start_with?("mini_racer") }.uniq.sort
  end

  def c_keywords
    source = Dir[File.join(UPSTREAM, "ext", "**", "*.c")].map { |file| File.read(file) }.join("\n")
    functions = source.scan(FUNCTION).to_h { |name, _args, body| [name, body] }
    bound = methods(source)
    bound.flat_map do |method, function|
      keywords(function, functions, bound.values).map { |keyword| "kwarg:MiniRacer::#{method}:#{keyword}" }
    end.uniq.sort
  end

  # Ruby method ("Context#eval") to the C function that implements it, tracking
  # which class each C variable holds as the Init function reassigns them.
  def methods(source)
    classes = {}
    source.each_line.with_object({}) do |line, out|
      if (match = line.match(DEFINE_CLASS))
        line.scan(/(\w+)\s*=/).flatten.each { |var| classes[var] = match[2] }
      elsif (match = line.match(DEFINE_METHOD)) && classes[match[1]]
        out["#{classes[match[1]]}##{match[2]}"] = match[3]
      end
    end
  end

  # Follows helpers such as context_eval_common, but not into a function bound
  # to another Ruby method: Snapshot#warmup! calling context_initialize does
  # not take Context.new's keywords.
  def keywords(function, functions, bound, seen = {})
    return [] if seen[function] || !functions.key?(function)

    seen[function] = true
    body = functions.fetch(function)
    direct = body.match?(/rb_scan_args\([^;]*"[^"]*:"/) ? scan_keywords(body) : []
    helpers = body.scan(/\b(\w+)\(/).flatten.uniq.select { |name| functions.key?(name) && !bound.include?(name) }
    direct + helpers.flat_map { |helper| keywords(helper, functions, bound, seen) }
  end

  def scan_keywords(body)
    KEYWORD_PATTERNS.flat_map { |pattern| body.scan(pattern).flatten }
  end
end

if $PROGRAM_NAME == __FILE__
  require "json"
  puts JSON.pretty_generate({ "features" => CompatExtract.required_features, "c_keywords" => CompatExtract.c_keywords })
end
