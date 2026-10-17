import { build } from "esbuild";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "../../../tmp/consumers/bundles");
mkdirSync(out, { recursive: true });

copyFileSync(join(here, "node_modules/@babel/standalone/babel.min.js"), join(out, "babel.js"));
copyFileSync(join(here, "node_modules/handlebars/dist/handlebars.min.js"), join(out, "handlebars.js"));
copyFileSync(join(here, "node_modules/bootstrap/dist/css/bootstrap.css"), join(out, "bootstrap.css"));

// @emotion/server imports these Node stream packages for renderStylesToNodeStream
// alone, which a string render never calls, and plain V8 has no stream module.
const unusedStreams = {
  name: "unused-streams",
  setup(build) {
    build.onResolve({ filter: /^(through|html-tokenize|multipipe)$/ }, ({ path }) => ({ path, namespace: "unused" }));
    build.onLoad({ filter: /.*/, namespace: "unused" }, () => ({ contents: "" }));
  },
};

const common = { bundle: true, format: "iife", platform: "neutral", minify: true, logLevel: "warning" };
await build({ ...common, entryPoints: [join(here, "entries/terser.js")], outfile: join(out, "terser.js"), mainFields: ["module", "main"] });
await build({
  ...common,
  entryPoints: [join(here, "entries/react.js")],
  outfile: join(out, "react.js"),
  mainFields: ["module", "main"],
  define: { "process.env.NODE_ENV": '"production"' },
});
// The browser platform applies the browser fields of browserslist and postcss,
// which leave fs and path out. autoprefixer reads AUTOPREFIXER_GRID from
// process.env and plain V8 has no process, so it reads as unset, as in Node.
await build({
  ...common,
  entryPoints: [join(here, "entries/autoprefixer.js")],
  outfile: join(out, "autoprefixer.js"),
  platform: "browser",
  minify: false,
  define: { "process.env.NODE_ENV": '"production"', "process.env.AUTOPREFIXER_GRID": "undefined" },
});
await build({
  ...common,
  entryPoints: [join(here, "entries/mui.js")],
  outfile: join(out, "mui.js"),
  mainFields: ["module", "main"],
  minify: false,
  define: { "process.env.NODE_ENV": '"production"' },
  plugins: [unusedStreams],
});

const inputs = ["build.mjs", "package-lock.json", ...readdirSync(join(here, "entries")).sort().map((name) => `entries/${name}`)];
const digest = createHash("sha256");
for (const file of inputs) digest.update(`${file}\0`).update(readFileSync(join(here, file))).update("\0");
writeFileSync(join(out, "inputs.sha256"), `${digest.digest("hex")}\n`);
