// Synthetic minimal reproduction of the Boa inline-cache prototype-slot
// poisoning tracked in Omoikane issue 058. This file is hand-written and
// contains no third-party code or credentials.
//
// Root cause (Boa 0.21.1): the property inline cache stores, for a
// prototype-chain hit, the *slot index into the prototype's storage*, but the
// fast-path guard only checks the *receiver's* shape. Deleting properties that
// sit *before* the cached one in the prototype's storage compacts that storage
// (surviving indices shift toward 0). The receiver's shape is untouched, so the
// warm call site keeps hitting the cache and now reads a stale index that lands
// on a *different* property.
//
// This mirrors the real symptom on tokyo6.tokyo: after a warm
// `"s".codePointAt(0)` call site, core-js deletes/redefines several
// `String.prototype` methods stored before `codePointAt`, shifting its slot so
// the stale index resolves to `concat` and `"s".codePointAt(0)` returns the
// string `"s0"` (== `"s".concat(0)`) instead of the number `115`.
//
// Here `target` plays the role of `codePointAt` (returns 115) and `d` plays the
// role of `concat` (returns "s0"). Property values are functions so the warm
// site is a method *call*, exactly like `codePointAt(0)`.

(function () {
  var proto = {};
  // Storage order is definition order: a=0, b=1, target=2, c=3, d=4.
  proto.a = function () { return 1; };
  proto.b = function () { return 2; };
  proto.target = function () { return 115; }; // victim, mimics codePointAt -> 115
  proto.c = function () { return 3; };
  proto.d = function () { return "s0"; };      // mimics concat("s", 0) === "s0"

  var receiver = Object.create(proto);

  // The victim call site. Exposed globally so the Rust test can re-invoke the
  // *same* warmed bytecode after the prototype is reindexed.
  function victim(obj) { return obj.target(); }
  globalThis.__issue058Victim = function () { return victim(receiver); };

  // Warm the `obj.target` inline cache. The slot caches prototype storage
  // index 2.
  var warm = 0;
  for (var i = 0; i < 8; i++) { warm = victim(receiver); }
  if (warm !== 115) {
    throw new Error("warmup should read 115, got " + warm);
  }

  // Reindex the prototype: delete two properties positioned *before* `target`,
  // then redefine them (as core-js does). Deleting `a` then `b` compacts
  // storage so `target` moves to index 0 and the stale cached index 2 now holds
  // `d` (=== "s0"). Re-adding `a`/`b` appends them at the tail, so `target`
  // stays at index 0 and index 2 keeps pointing at `d`. `receiver`'s shape is
  // unaffected throughout.
  delete proto.a;
  delete proto.b;
  proto.a = function () { return 111; };
  proto.b = function () { return 222; };

  // Sentinel: the mutation completed without throwing. The Rust test asserts
  // this is `true` before re-invoking the warm call site, so an early throw
  // (which would leave the prototype un-mutated) cannot make the test
  // false-pass.
  globalThis.__issue058Mutated = true;
})();
