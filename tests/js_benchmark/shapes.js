// Workload shapes for the JavaScript execution benchmark.
//
// Each shape isolates one cost centre of the engine, because the interesting
// number is not "how fast is JS" but "which part of the engine is slow". The
// shapes were chosen from a Boa/SpiderMonkey comparison (issue #305) where the
// gap ranged from 1.7x on monomorphic property access to 15.4x on string
// building against SpiderMonkey's interpreter.
//
// This file is executed verbatim in other engines to produce the reference
// numbers in `baseline.json`, so it must stay free of Omoikane-specific APIs
// and of syntax newer than the oldest engine being compared.
//
// The iteration counts are sized for the *reference* engine, not for Boa. An
// interpreter's cost per operation barely moves with the count (Boa measured
// 128.0 ns/op at 2,000,000 iterations against 127.6 at 400,000), but a tiering
// JIT needs enough iterations to reach its optimizing tier — at a fifth of these
// counts SpiderMonkey reported 8.30 ns/op for `arith` instead of 3.99. Lowering
// them would make the recorded ratios compare a warmed interpreter against a
// cold JIT.

// Every shape feeds its result into this sink so no engine can discard the loop
// as dead code.
globalThis.__benchSink = 0;

// Each shape is timed several times and the *fastest* pass is reported. Two
// reasons:
//
// - A tiered engine gets a warmed tier, and an interpreter is not charged for a
//   cold first pass either.
// - Competing load on the machine can only make a pass slower, never faster, so
//   the minimum is the right estimator for "how fast can this machine do it" and
//   raising the pass count monotonically improves noise resistance. One
//   unimpeded pass is enough for the minimum to be sound. Four passes were
//   chosen after a build competing for a 2-core container inflated every shape
//   by 21-54% at two passes.
var BENCH_PASSES = 4;

function bench(name, iterations, body) {
  var best = Infinity;
  for (var pass = 0; pass < BENCH_PASSES; pass++) {
    var start = performance.now();
    var result = body(iterations);
    var elapsed = performance.now() - start;
    globalThis.__benchSink =
      (globalThis.__benchSink + (typeof result === "number" ? result : 1)) % 1000003;
    if (elapsed < best) best = elapsed;
  }
  return name + "|" + iterations + "|" + best.toFixed(4) + "|" + ((best * 1e6) / iterations).toFixed(2);
}

// Eight objects with distinct property insertion orders, so they occupy
// distinct shapes/hidden classes.
function makeShapes() {
  var shapes = [];
  for (var i = 0; i < 8; i++) {
    var object = {};
    for (var j = 0; j <= i; j++) object["f" + j] = j;
    object.value = i;
    shapes.push(object);
  }
  return shapes;
}

globalThis.runBenchmarks = function () {
  var lines = [];

  // Bytecode dispatch and integer arithmetic, with no property access or
  // allocation in the loop.
  lines.push(bench("arith", 2000000, function (n) {
    var s = 1;
    for (var i = 0; i < n; i++) s = (s + i * 3) % 1000003;
    return s;
  }));

  // Monomorphic property read/write: one shape through one call site, which is
  // the inline cache hit path.
  lines.push(bench("prop-mono", 1000000, function (n) {
    var o = { a: 1, b: 2, c: 3 };
    var s = 0;
    for (var i = 0; i < n; i++) { o.b = o.a + i; s += o.b + o.c; }
    return s;
  }));

  // Megamorphic property read: eight shapes through one call site, which is the
  // inline cache miss path.
  lines.push(bench("prop-mega", 500000, function (n) {
    var shapes = makeShapes();
    var s = 0;
    for (var i = 0; i < n; i++) s += shapes[i & 7].value;
    return s;
  }));

  // Function call overhead.
  lines.push(bench("call", 1000000, function (n) {
    function add(a, b) { return a + b; }
    var s = 0;
    for (var i = 0; i < n; i++) s = add(s, 1) % 1000003;
    return s;
  }));

  // One closure allocated per iteration: allocator and GC pressure.
  lines.push(bench("closure-alloc", 300000, function (n) {
    var s = 0;
    for (var i = 0; i < n; i++) {
      var f = function (x) { return x + i; };
      s = f(s) % 1000003;
    }
    return s;
  }));

  // One object literal allocated per iteration: allocator and GC pressure.
  lines.push(bench("object-alloc", 300000, function (n) {
    var s = 0;
    for (var i = 0; i < n; i++) { var o = { x: i, y: i + 1 }; s = (s + o.x + o.y) % 1000003; }
    return s;
  }));

  // String building: string representation and reallocation rather than
  // dispatch.
  lines.push(bench("string-concat", 200000, function (n) {
    var s = "";
    for (var i = 0; i < n; i++) { s += "ab"; if (s.length > 4096) s = ""; }
    return s.length;
  }));

  // Array index and push.
  lines.push(bench("array", 500000, function (n) {
    var a = [];
    var s = 0;
    for (var i = 0; i < n; i++) {
      a.push(i);
      s += a[i & 1023];
      if (a.length > 1024) a.length = 0;
    }
    return s;
  }));

  // Method call resolved through a prototype. This is the shape that Boa's
  // inline cache poisoning bug corrupted (issues #057/#058), so a regression
  // here is worth noticing.
  lines.push(bench("proto-method", 500000, function (n) {
    var s = 0;
    for (var i = 0; i < n; i++) s = (s + "abc".charCodeAt(i & 2)) % 1000003;
    return s;
  }));

  return lines.join("\n");
};
