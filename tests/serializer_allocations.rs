//! Serializer allocation evidence. Inputs and equality checks are outside measurement.
//! Requested bytes and peak live requests are not RSS or elapsed time.
use omoikane::dom::NodeHandle;
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
        "SERIALIZER_ALLOCATION kind={kind} items={items} input_bytes={input_bytes} allocated_bytes={} allocation_requests={} baseline_live_bytes={} peak_live_bytes={} peak_additional_bytes={}",
        sample.allocated,
        sample.requests,
        sample.baseline,
        sample.peak,
        sample.peak.saturating_sub(sample.baseline),
    );
}

fn xml_sample(kind: &str, items: usize) {
    let value = "aé😀".repeat(items);
    let host = NodeHandle::element("div");
    let expected = match kind {
        "xml-text" => {
            host.append_child(NodeHandle::text(&value));
            format!("<div xmlns=\"http://www.w3.org/1999/xhtml\">{value}</div>")
        }
        "xml-attribute" => {
            host.set_attribute("data-value", &value);
            format!("<div xmlns=\"http://www.w3.org/1999/xhtml\" data-value=\"{value}\"></div>")
        }
        _ => {
            for _ in 0..8 {
                let child = NodeHandle::element("p");
                child.set_attribute("data-value", &value);
                child.append_child(NodeHandle::text(&value));
                host.append_child(child);
            }
            let child = format!("<p data-value=\"{value}\">{value}</p>");
            format!(
                "<div xmlns=\"http://www.w3.org/1999/xhtml\">{}</div>",
                child.repeat(8)
            )
        }
    };
    let (output, sample) = measure(|| omoikane::xml::serialize_utf16(&host));
    assert_eq!(output, expected.encode_utf16().collect::<Vec<_>>());
    report(kind, items, value.len(), &sample);
}

fn html_sample(kind: &str, items: usize) {
    let document = TreeBuilder::parse("<body>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.eval(&format!(r#"
        globalThis.host = document.createElement('div');
        globalThis.payload = 'aé😀'.repeat({items});
        if ('{kind}' === 'html-text') host.append(document.createTextNode(payload));
        else if ('{kind}' === 'html-attribute') host.setAttribute('data-value', payload);
        else for (let i=0; i<8; i++) {{
            const p=document.createElement('p');
            p.setAttribute('data-value', payload); p.append(document.createTextNode(payload)); host.append(p);
        }}
        globalThis.serializeSample = () => host.outerHTML;
    "#)).unwrap();
    runtime.eval("serializeSample()").unwrap();
    let value = "aé😀".repeat(items);
    let expected = match kind {
        "html-text" => format!("<div>{value}</div>"),
        "html-attribute" => format!("<div data-value=\"{value}\"></div>"),
        _ => format!(
            "<div>{}</div>",
            format!("<p data-value=\"{value}\">{value}</p>").repeat(8)
        ),
    };
    let (output, sample) = measure(|| runtime.eval("serializeSample()"));
    assert_eq!(
        output
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string()
            .unwrap(),
        expected
    );
    report(kind, items, items * "aé😀".len(), &sample);
}

#[test]
fn report_n_and_2n_serialization_allocations() {
    for items in [8, 16_384, 32_768] {
        for kind in ["xml-text", "xml-attribute", "xml-subtree"] {
            xml_sample(kind, items);
        }
        for kind in ["html-text", "html-attribute", "html-subtree"] {
            html_sample(kind, items);
        }
    }
}
