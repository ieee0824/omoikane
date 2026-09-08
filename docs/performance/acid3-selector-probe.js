// Evaluate after the Acid3 page scripts initialize, before driving update().
// Use a fresh runtime for each sample. Read JSON.stringify(selectorSamples)
// after the direct driver completes.
globalThis.selectorSamples = [];
const originalSelectorTest = selectorTest;
selectorTest = function(tester) {
  const start = performance.now();
  let checks = 0;
  try {
    return originalSelectorTest(function(doc, add, expect) {
      return tester(doc, add, function(...args) {
        checks++;
        return expect(...args);
      });
    });
  } finally {
    selectorSamples.push({ test: index, elapsedMs: performance.now() - start, checks });
  }
};
