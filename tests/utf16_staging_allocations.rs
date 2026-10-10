//! Deterministic allocation evidence for Issue #1328.
//!
//! Run this unchanged on both revisions with the same toolchain/profile:
//! `cargo test --locked --test utf16_staging_allocations -- --nocapture`.
//! Counts are allocator-requested bytes, including realloc requests, and peak
//! live requested bytes. They exclude allocator metadata and the source input
//! prepared before measurement. No elapsed-time assertion is used.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

struct RecordingAllocator;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOCATION_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static RECORDING: AtomicBool = AtomicBool::new(false);

fn allocated(bytes: usize) {
    let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    if RECORDING.load(Ordering::Relaxed) {
        ALLOCATED_BYTES.fetch_add(bytes, Ordering::Relaxed);
        ALLOCATION_REQUESTS.fetch_add(1, Ordering::Relaxed);
        PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
    }
}

// SAFETY: Every operation delegates to System with the original pointer and
// layout. The counters neither inspect nor retain allocated memory.
unsafe impl GlobalAlloc for RecordingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if !replacement.is_null() {
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
            allocated(size);
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: RecordingAllocator = RecordingAllocator;

#[derive(Debug)]
struct AllocationSample {
    allocated: usize,
    requests: usize,
    baseline: usize,
    peak: usize,
}

fn measure<T>(operation: impl FnOnce() -> T) -> (T, AllocationSample) {
    let baseline = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    ALLOCATION_REQUESTS.store(0, Ordering::Relaxed);
    RECORDING.store(true, Ordering::Relaxed);
    let result = operation();
    RECORDING.store(false, Ordering::Relaxed);
    let sample = AllocationSample {
        allocated: ALLOCATED_BYTES.load(Ordering::Relaxed),
        requests: ALLOCATION_REQUESTS.load(Ordering::Relaxed),
        baseline,
        peak: PEAK_BYTES.load(Ordering::Relaxed),
    };
    (result, sample)
}

fn report(kind: &str, items: usize, input_bytes: usize, sample: &AllocationSample) {
    eprintln!(
        "UTF16_ALLOCATION kind={kind} items={items} input_bytes={input_bytes} allocated_bytes={} allocation_requests={} baseline_live_bytes={} peak_live_bytes={} peak_additional_bytes={}",
        sample.allocated,
        sample.requests,
        sample.baseline,
        sample.peak,
        sample.peak.saturating_sub(sample.baseline),
    );
}

fn html_sample(items: usize) {
    let value = "aé😀".repeat(items);
    let source =
        format!("<!doctype html><body><p x=\"{value}\">{value}<!--{value}--><?probe {value}?></p>");
    let (parsed, sample) = measure(|| TreeBuilder::parse(&source));
    assert!(parsed.errors().is_empty());
    report("html", items, source.len(), &sample);
}

fn dom_sample(items: usize) {
    let document = TreeBuilder::parse("<body>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval(&format!("globalThis.payload = 'aé😀'.repeat({items});"))
        .unwrap();
    const SCRIPT: &str = r#"(() => {
        const value = payload, host = document.createElement('div');
        const text = document.createTextNode(value), comment = document.createComment(value);
        const instruction = document.createProcessingInstruction('probe', value);
        text.data = value; comment.data = value; instruction.data = value;
        host.textContent = value;
        host.setAttribute('data-value', value);
        host.setAttributeNS('urn:probe', 'p:value', value);
        host.setHTMLUnsafe('<p x="' + value + '">' + value + '</p>');
        document.write('<p id=written x="' + value + '">' + value + '</p>');
        document.close();
        return host.firstChild.textContent === value &&
            document.getElementById('written').textContent === value;
    })()"#;
    let (result, sample) = measure(|| runtime.eval(SCRIPT));
    assert_eq!(result.unwrap().as_boolean(), Some(true));
    report("dom", items, items * "aé😀".len(), &sample);
}

#[test]
fn report_n_and_2n_scalar_allocation_work() {
    // Initialize parser and JavaScript runtime state before the recorded sizes.
    html_sample(8);
    dom_sample(8);
    for items in [16_384, 32_768] {
        html_sample(items);
        dom_sample(items);
    }
}
