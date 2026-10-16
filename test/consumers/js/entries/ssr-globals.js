// react-dom/server.browser reaches for MessageChannel and TextEncoder when it
// loads; plain V8 has neither, so SSR setups on mini_racer provide stand-ins.
globalThis.MessageChannel ??= class MessageChannel {
  constructor() {
    this.port1 = { onmessage: null, postMessage() {} };
    this.port2 = { postMessage: (data) => this.port1.onmessage?.({ data }) };
  }
};

globalThis.TextEncoder ??= class TextEncoder {
  get encoding() {
    return "utf-8";
  }

  encode(input = "") {
    const bytes = [];
    for (const char of String(input)) {
      let code = char.codePointAt(0);
      if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd;
      if (code < 0x80) bytes.push(code);
      else if (code < 0x800) bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
      else if (code < 0x10000) bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
      else bytes.push(0xf0 | (code >> 18), 0x80 | ((code >> 12) & 0x3f), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
    }
    return new Uint8Array(bytes);
  }
};
