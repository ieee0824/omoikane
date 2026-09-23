// Temporary diagnostic for issue #785. Exercise one real test26 iteration's
// operations in the loaded Acid3 document and aggregate 128 repetitions.
(() => {
  const phases = ["date_and_text", "create_element", "append_text", "bucket1_lookup",
                  "insert_before", "bucket2_lookup_and_traversal", "set_attribute", "remove_child"];
  const totals = phases.map(() => 0);
  const samples = 128;
  const started = performance.now();
  for (let i = 0; i < samples; i++) {
    let start = performance.now();
    let date = new Date();
    date = new (function (value) { return { toString: function () { return value.toString(); } }; })(date.valueOf());
    const text = document.createTextNode("iteration " + i + " at " + date);
    totals[0] += performance.now() - start;

    start = performance.now();
    const anchor = document.createElement("a");
    totals[1] += performance.now() - start;

    start = performance.now();
    anchor.appendChild(text);
    totals[2] += performance.now() - start;

    start = performance.now();
    const before = document.getElementById("bucket1").parentNode;
    totals[3] += performance.now() - start;

    start = performance.now();
    document.body.insertBefore(anchor, before);
    totals[4] += performance.now() - start;

    start = performance.now();
    if (!document.getElementById("bucket2").nextSibling.parentNode.previousSibling.firstChild.data.match(/AT\W/i)) {
      throw new Error("test26 traversal probe failed");
    }
    totals[5] += performance.now() - start;

    start = performance.now();
    anchor.setAttribute("class", anchor.textContent);
    totals[6] += performance.now() - start;

    start = performance.now();
    document.body.removeChild(anchor);
    totals[7] += performance.now() - start;
  }
  const elapsed = performance.now() - started;
  const result = {};
  for (let i = 0; i < phases.length; i++) result[phases[i]] = Math.round(totals[i] * 1000) / 1000;
  return JSON.stringify({ samples, elapsed_ms: elapsed, phases_ms: result });
})()
