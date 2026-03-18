const path = require("node:path");
const ffi = require("ffi-napi");
const ref = require("ref-napi");

const lib = ffi.Library(
  path.join(__dirname, "..", "..", "target", "debug", "libomoikane"),
  {
    omoikane_init: ["pointer", []],
    omoikane_navigate: ["bool", ["pointer", "string"]],
    omoikane_evaluate: ["pointer", ["pointer", "string"]],
    omoikane_string_free: ["void", ["pointer"]],
    omoikane_free: ["void", ["pointer"]],
  },
);

const browser = lib.omoikane_init();
lib.omoikane_navigate(
  browser,
  "data:text/html,<html><body><main id='app'>node</main></body></html>",
);
const resultPtr = lib.omoikane_evaluate(browser, "document.getElementById('app').nodeName");
console.log(ref.readCString(resultPtr, 0));
lib.omoikane_string_free(resultPtr);
lib.omoikane_free(browser);
