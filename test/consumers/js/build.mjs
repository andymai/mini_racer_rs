import { build } from "esbuild";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "../../../tmp/consumers/bundles");
mkdirSync(out, { recursive: true });

copyFileSync(join(here, "node_modules/@babel/standalone/babel.min.js"), join(out, "babel.js"));
copyFileSync(join(here, "node_modules/handlebars/dist/handlebars.min.js"), join(out, "handlebars.js"));

const common = { bundle: true, format: "iife", platform: "neutral", minify: true, logLevel: "warning" };
await build({ ...common, entryPoints: [join(here, "entries/terser.js")], outfile: join(out, "terser.js"), mainFields: ["module", "main"] });
await build({
  ...common,
  entryPoints: [join(here, "entries/react.js")],
  outfile: join(out, "react.js"),
  mainFields: ["module", "main"],
  define: { "process.env.NODE_ENV": '"production"' },
});
