# frozen_string_literal: true

require "json"

# Real libraries run the way applications use mini_racer: a bundle loaded once,
# then work done in the same context through eval, Context#call, or ExecJS.
# A step naming a bundle gets that file's source; bundles come from
# `npm run build` in test/consumers/js.
module DiffConsumers
  BUNDLES = File.expand_path("../../tmp/consumers/bundles", __dir__)
  JS = File.expand_path("js", __dir__)

  def self.text(value) = ["string", "UTF-8", "text", value]

  def self.evaluate(source, setup: false) = { "op" => "eval", "source" => source, "setup" => setup }

  def self.load_bundle(bundle) = { "op" => "eval", "bundle" => bundle, "setup" => true }

  WORKLOADS = {
    "babel-transform" => [
      load_bundle("babel"),
      evaluate(<<~JS),
        Babel.transform("const f = (a) => a ?? 1; class C { #x = 1; get x() { return this.#x } }",
                        { presets: [["env", { targets: "ie 11" }]] }).code
      JS
    ],
    "babel-jsx" => [
      load_bundle("babel"),
      evaluate(%(Babel.transform("const a = <a href={u}>{t}</a>", { presets: ["react"] }).code)),
    ],
    "terser-minify" => [
      load_bundle("terser"),
      evaluate(%(Terser.minify_sync("function add(first, second) { return first + second } add(1, 2)").code)),
    ],
    "terser-via-execjs" => [
      { "op" => "execjs", "bundle" => "terser", "function" => "minify",
        "append" => "function minify(code) { return Terser.minify_sync(code).code }",
        "args" => [text("var answer = 40 + 2; console.log(answer)")], },
    ],
    "handlebars-render" => [
      load_bundle("handlebars"),
      evaluate(<<~JS),
        Handlebars.compile("<p>{{#each people}}{{name}}, {{/each}}</p>")({ people: [{ name: "a" }, { name: "<b>" }] })
      JS
    ],
    "handlebars-call" => [
      load_bundle("handlebars"),
      evaluate(%(globalThis.render = Handlebars.compile("{{title}}: {{#each tags}}[{{this}}]{{/each}}")), setup: true),
      { "op" => "call", "path" => "render",
        "args" => [["hash", [[text("title"), text("Café")], [text("tags"), ["array", [text("x"), text("<y>")]]]]]], },
    ],
    "react-ssr" => [
      load_bundle("react"),
      evaluate(%(renderApp({ name: "w\\u00f6rld", items: ["x", "<y>"] }))),
    ],
    "react-ssr-call" => [
      load_bundle("react"),
      { "op" => "call", "path" => "renderApp",
        "args" => [["hash", [[text("name"), text("world")], [text("items"), ["array", [text("a")]]]]]], },
    ],
  }.freeze

  module_function

  def cases
    bundles = WORKLOADS.values.flatten.filter_map { |step| step["bundle"] }.uniq
    missing = bundles.reject { |name| File.exist?(bundle(name)) }
    raise "missing consumer bundles #{missing.join(", ")}; run `npm ci && npm run build` in #{JS}" unless missing.empty?

    WORKLOADS.map do |name, steps|
      { "id" => "consumer-#{name}", "kind" => "program", "steps" => steps.map { |step| resolve(step) } }
    end
  end

  def bundle(name)
    File.join(BUNDLES, "#{name}.js")
  end

  def resolve(step)
    return step unless step.key?("bundle")

    source = [File.read(bundle(step["bundle"])), step["append"]].compact.join("\n")
    step.except("bundle", "append").merge("source" => source)
  end
end
