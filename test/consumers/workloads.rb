# frozen_string_literal: true

require "digest"

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
    # The 8 MB bundle is two bytes a unit in V8. Loaded once per workload, it
    # is read from one shared copy in each of them, the first included.
    "mui-dashboard" => [
      load_bundle("mui"),
      evaluate("renderDashboard(2)"),
    ],
    "mui-dashboard-source" => [
      load_bundle("mui"),
      evaluate("renderDashboard.toString()"),
    ],
    "mui-dashboard-stack" => [
      load_bundle("mui"),
      evaluate("(() => { try { renderDashboard(Symbol()) } catch (e) { return [e.name, e.stack] } })()"),
    ],
  }.freeze

  module_function

  def cases
    bundles = WORKLOADS.values.flatten.filter_map { |step| step["bundle"] }.uniq
    missing = bundles.reject { |name| File.exist?(bundle(name)) }
    raise "missing consumer bundles #{missing.join(", ")}; run `npm ci && npm run build` in #{JS}" unless missing.empty?
    raise "consumer bundles are older than their sources in #{JS}; run `npm ci && npm run build` there" unless current?

    WORKLOADS.map do |name, steps|
      { "id" => "consumer-#{name}", "kind" => "program", "steps" => steps.map { |step| resolve(step) } }
    end
  end

  def bundle(name)
    File.join(BUNDLES, "#{name}.js")
  end

  # Bundles are build output in tmp/, so nothing else notices when their
  # inputs change; build.mjs stamps the digest of the same files, in the same order.
  def current?
    stamp = File.join(BUNDLES, "inputs.sha256")
    File.exist?(stamp) && File.read(stamp).strip == inputs_digest
  end

  def inputs_digest
    files = ["build.mjs", "package-lock.json", *Dir.children(File.join(JS, "entries")).sort.map do |name|
      "entries/#{name}"
    end,]
    digest = Digest::SHA256.new
    files.each { |file| digest << file << "\0" << File.binread(File.join(JS, file)) << "\0" }
    digest.hexdigest
  end

  def resolve(step)
    return step unless step.key?("bundle")

    step.except("bundle").merge("file" => bundle(step["bundle"]))
  end
end
