import "./ssr-globals.js";
import { createElement } from "react";
import { renderToString } from "react-dom/server.browser";

function App({ name, items }) {
  return createElement(
    "main",
    null,
    createElement("h1", null, `Hello ${name}`),
    createElement("ul", null, items.map((item) => createElement("li", { key: item }, item))),
  );
}

globalThis.renderApp = (props) => renderToString(createElement(App, props));
