import autoprefixer from "autoprefixer";
import postcss from "postcss";

// Browserslist's defaults, given explicitly so no configuration is looked up.
const processor = postcss([autoprefixer({ overrideBrowserslist: ["> 0.5%", "last 2 versions", "Firefox ESR", "not dead"] })]);

globalThis.prefix = (css) => processor.process(css, { from: undefined }).css;
