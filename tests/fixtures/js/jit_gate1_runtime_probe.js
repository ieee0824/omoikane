// Public-embedding probe for issue #307 Gate 1.
//
// This deliberately exercises the same semantic shapes that a future
// interpreter/JIT boundary must preserve. It does not pretend that a JS
// string can inspect Boa's private bytecode or native stack; the Rust test
// drives the Omoikane runtime and forces collection between the phases.

(() => {
  const result = Object.create(null);

  // Arithmetic / dispatch shape.
  let arithmetic = 0;
  for (let i = 0; i < 100; i++) arithmetic += i;
  result.arithmetic = arithmetic;

  // Prototype property access, which warms the same kind of mono property
  // lookup that #057/#058 protect against stale prototype slots.
  const prototype = { value: 7 };
  const receiver = Object.create(prototype);
  let prototypeProperty = 0;
  for (let i = 0; i < 100; i++) prototypeProperty += receiver.value;
  result.prototypeProperty = prototypeProperty;

  // Shape transitions, delete/redefine, and a later property write.
  const transitioned = { a: 1 };
  transitioned.b = 2;
  transitioned.c = 3;
  const beforeMutation = transitioned.c;
  delete transitioned.b;
  Object.defineProperty(transitioned, "b", {
    configurable: true,
    enumerable: true,
    value: 11,
    writable: true,
  });
  transitioned.c = 13;
  result.shapeMutation = [beforeMutation, transitioned.b, transitioned.c];

  // Keep a key and its value reachable through a WeakMap. This is the
  // observable ephemeron case for the forced-GC part of the probe.
  const weakKey = {};
  const weakValue = { marker: "weak-value" };
  const weakMap = new WeakMap();
  weakMap.set(weakKey, weakValue);
  globalThis.__gate1WeakKey = weakKey;
  globalThis.__gate1WeakMap = weakMap;

  // The Rust test collects once before writing the young child, then again.
  // That is the old-to-young remembered-edge case.
  globalThis.__gate1Old = { children: [] };

  const rooted = { nested: { value: 42 }, array: [1, 2, 3] };
  globalThis.__gate1Root = rooted;
  Promise.resolve().then(() => {
    result.promise = rooted.nested.value + rooted.array.length;
  });
  setTimeout(() => {
    result.timer = result.promise + 1;
  }, 0);

  globalThis.__omoikane_gate1_result = result;
})();
