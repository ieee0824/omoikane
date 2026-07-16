use omoikane::html::TreeBuilder;
use omoikane::layout::Rect;
use omoikane::paint::render_document;

const FIXTURE: &str = include_str!("fixtures/anonymized-render-benchmark/page.html");

#[test]
fn render_benchmark_fixture_is_deterministic() {
    let viewport = Rect { x: 0.0, y: 0.0, width: 1280.0, height: 720.0 };
    let first_document = TreeBuilder::parse(FIXTURE).document();
    let second_document = TreeBuilder::parse(FIXTURE).document();

    let first = render_document(&first_document, viewport).expect("first render should succeed");
    let second = render_document(&second_document, viewport).expect("second render should succeed");

    assert_eq!(first.width(), 1280);
    assert_eq!(first.height(), 720);
    assert_eq!(first.encode_png(), second.encode_png());
}
