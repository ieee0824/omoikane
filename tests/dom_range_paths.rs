use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn deep_range_comparison_does_not_shift_ancestor_arrays() {
    for depth in [32, 64, 128, 256] {
        let branch = |id: &str| {
            format!(
                "{}<span id='{id}'>text</span>{}",
                "<div>".repeat(depth),
                "</div>".repeat(depth)
            )
        };
        let mut runtime = JsRuntime::with_document(
            TreeBuilder::parse(&format!(
                "<main>{}{}</main>",
                branch("left"),
                branch("right")
            ))
            .document(),
        )
        .unwrap();
        assert_eq!(runtime.eval(r#"(() => {
            const left = document.createRange(), right = document.createRange();
            left.selectNodeContents(document.getElementById('left'));
            right.selectNodeContents(document.getElementById('right'));
            if (left.compareBoundaryPoints(Range.START_TO_START, right) !== -1) return false;
            let shifts = 0;
            const original = Array.prototype.unshift;
            Array.prototype.unshift = function(...values) {
                shifts += this.length;
                return Reflect.apply(original, this, values);
            };
            try {
                return [Range.START_TO_START, Range.START_TO_END, Range.END_TO_END, Range.END_TO_START]
                    .every(mode => left.compareBoundaryPoints(mode, right) === -1 &&
                        right.compareBoundaryPoints(mode, left) === 1) && shifts === 0;
            } finally { Array.prototype.unshift = original; }
        })()"#).unwrap().as_boolean(), Some(true), "depth {depth}");
    }
}

#[test]
fn range_comparison_preserves_offsets_ancestors_and_separate_roots() {
    let mut runtime = JsRuntime::with_document(
        TreeBuilder::parse("<main><span id='left'>abcd</span><span id='right'>efgh</span></main>")
            .document(),
    )
    .unwrap();
    assert_eq!(
        runtime
            .eval(
                r#"(() => {
        const span = document.getElementById('left'), text = span.firstChild;
        const a = document.createRange(), b = document.createRange();
        a.setStart(text, 1); a.setEnd(text, 3);
        b.setStart(text, 2); b.setEnd(text, 4);
        if (a.compareBoundaryPoints(Range.START_TO_START, b) !== -1 ||
            a.compareBoundaryPoints(Range.START_TO_END, b) !== 1 ||
            a.compareBoundaryPoints(Range.END_TO_END, b) !== -1 ||
            a.compareBoundaryPoints(Range.END_TO_START, b) !== -1) return false;
        a.selectNodeContents(span); b.selectNodeContents(text);
        if (a.compareBoundaryPoints(Range.START_TO_START, b) !== -1 ||
            a.compareBoundaryPoints(Range.END_TO_END, b) !== 1) return false;
        a.selectNodeContents(text);
        if (a.compareBoundaryPoints(Range.START_TO_START, b) !== 0) return false;
        const other = document.implementation.createHTMLDocument('other');
        b.selectNodeContents(other.body);
        return a.compareBoundaryPoints(Range.START_TO_START, b) === 0;
    })()"#
            )
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
