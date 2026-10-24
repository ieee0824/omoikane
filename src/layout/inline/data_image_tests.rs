use super::*;

#[test]
fn static_svg_data_image_reuses_rasterized_pixels() {
    let uri = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='4' height='3'%3E%3Crect width='4' height='3' fill='red'/%3E%3C/svg%3E";
    IMAGE_CACHE.with(|cache| cache.borrow_mut().remove(uri));
    let first = decode_data_uri_image(uri).unwrap();
    assert!(IMAGE_CACHE.with(|cache| cache.borrow().contains_key(uri)));
    let second = decode_data_uri_image(uri).unwrap();
    assert_eq!(first, second);
    assert_eq!((first.width(), first.height()), (4, 3));
    assert_eq!(first.pixels(), &[255, 0, 0, 255].repeat(12));
}

#[test]
fn svg_data_image_with_nested_resources_is_not_cached() {
    // Even a currently empty image can acquire a resource in supported SVG
    // rendering paths; do not freeze its first rasterization.
    let uri = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='4' height='3'%3E%3Cg%3E%3Cimage width='4' height='3'/%3E%3C/g%3E%3C/svg%3E";
    IMAGE_CACHE.with(|cache| cache.borrow_mut().remove(uri));
    assert!(decode_data_uri_image(uri).is_some());
    assert!(!IMAGE_CACHE.with(|cache| cache.borrow().contains_key(uri)));
}

#[test]
fn invalid_svg_data_image_is_not_cached() {
    let uri = "data:image/svg+xml,not-an-svg";
    assert!(decode_data_uri_image(uri).is_none());
    assert!(!IMAGE_CACHE.with(|cache| cache.borrow().contains_key(uri)));
}

#[test]
fn base64_svg_data_image_is_cached() {
    use base64::Engine as _;
    let text = "<svg xmlns='http://www.w3.org/2000/svg' width='2' height='2'><rect width='2' height='2' fill='blue'/></svg>";
    let uri = format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(text)
    );
    IMAGE_CACHE.with(|cache| cache.borrow_mut().remove(&uri));
    let image = decode_data_uri_image(&uri).unwrap();
    assert_eq!(image.pixels(), &[0, 0, 255, 255].repeat(4));
    assert!(IMAGE_CACHE.with(|cache| cache.borrow().contains_key(&uri)));
}

#[test]
fn static_svg_data_cache_has_an_entry_limit() {
    super::super::ensure_image_cache_context();
    IMAGE_CACHE.with(|cache| cache.borrow_mut().clear());
    for i in 0..=MAX_CACHED_DATA_SVGS {
        let uri = format!(
            "data:image/svg+xml,%3Csvg width='1' height='1'%3E%3Crect width='1' height='1' fill='red'/%3E%3C!--{i}--%3E%3C/svg%3E"
        );
        let image = decode_data_uri_image(&uri).unwrap();
        assert_eq!(image.pixels(), &[255, 0, 0, 255]);
        assert_eq!(
            IMAGE_CACHE.with(|cache| cache.borrow().contains_key(&uri)),
            i < MAX_CACHED_DATA_SVGS
        );
    }
    IMAGE_CACHE.with(|cache| cache.borrow_mut().clear());
}

#[test]
fn oversized_static_svg_is_rendered_without_caching() {
    let uri = "data:image/svg+xml,%3Csvg width='257' height='256'%3E%3Crect width='257' height='256' fill='red'/%3E%3C/svg%3E";
    let image = decode_data_uri_image(uri).unwrap();
    assert_eq!((image.width(), image.height()), (257, 256));
    assert!(!IMAGE_CACHE.with(|cache| cache.borrow().contains_key(uri)));
}
