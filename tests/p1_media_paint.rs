//! Used colors must follow the same forced-colors environment as media queries.
use omoikane::css::{MediaEnvironment, StyleResolver};
use omoikane::dom::Node;
use omoikane::html::TreeBuilder;
use omoikane::layout::{Rect, layout_tree};
use omoikane::paint::{Color, paint_layout};

#[test]
fn forced_colors_replace_used_backgrounds_and_respect_opt_out() {
    let document = TreeBuilder::parse(r#"<html style="margin:0"><body style="margin:0">
        <div style="position:absolute;left:0;top:0;width:20px;height:20px;background:red"></div>
        <div style="position:absolute;left:20px;top:0;width:20px;height:20px;background:red;forced-color-adjust:none"></div>
        </body></html>"#).document();
    let viewport = Rect {
        width: 60.0,
        height: 40.0,
        ..Default::default()
    };
    let mut resolver = StyleResolver::new();
    let normal_layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let normal = paint_layout(&normal_layout, &mut resolver, viewport);
    assert_eq!(normal.pixel(10, 10), Some(Color::rgb(255, 0, 0)));
    assert_eq!(normal.pixel(30, 10), Some(Color::rgb(255, 0, 0)));
    let mut environment = MediaEnvironment::default();
    assert!(environment.set_feature("forced-colors", "active"));
    resolver.set_media_environment(environment);
    let forced_layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let forced = paint_layout(&forced_layout, &mut resolver, viewport);
    assert_eq!(forced.pixel(10, 10), Some(Color::rgb(255, 255, 255)));
    assert_eq!(forced.pixel(30, 10), Some(Color::rgb(255, 0, 0)));
}

#[test]
fn forced_palette_is_owned_and_reset_restores_author_backgrounds() {
    use omoikane::css::ForcedColorPalette;
    let document = TreeBuilder::parse(
        "<div style='position:absolute;left:0;top:0;width:20px;height:20px;background:red'></div>",
    )
    .document();
    let viewport = Rect {
        width: 40.0,
        height: 40.0,
        ..Default::default()
    };
    let mut resolver = StyleResolver::new();
    let mut environment = MediaEnvironment::default();
    environment.set_feature("forced-colors", "active");
    let mut palette = ForcedColorPalette::for_color_scheme(false);
    palette.canvas = [11, 22, 33];
    environment.forced_color_palette = Some(palette);
    resolver.set_media_environment(environment);
    palette.canvas = [200, 0, 0];
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    assert_eq!(
        paint_layout(&layout, &mut resolver, viewport).pixel(10, 10),
        Some(Color::rgb(11, 22, 33))
    );
    resolver.set_media_environment(MediaEnvironment::default());
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    assert_eq!(
        paint_layout(&layout, &mut resolver, viewport).pixel(10, 10),
        Some(Color::rgb(255, 0, 0))
    );
    assert_eq!(palette.canvas, [200, 0, 0]);
}

#[test]
fn forced_color_opt_out_is_inherited_by_descendants() {
    let document = TreeBuilder::parse("<div style='forced-color-adjust:NONE'><div style='position:absolute;left:0;top:0;width:20px;height:20px;background:red'></div></div>").document();
    let viewport = Rect {
        width: 40.0,
        height: 40.0,
        ..Default::default()
    };
    let mut resolver = StyleResolver::new();
    let mut environment = MediaEnvironment::default();
    environment.set_feature("forced-colors", "active");
    resolver.set_media_environment(environment);
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    assert_eq!(
        paint_layout(&layout, &mut resolver, viewport).pixel(10, 10),
        Some(Color::rgb(255, 0, 0))
    );
}

#[test]
fn forced_paint_preserves_the_cached_computed_style() {
    let document = TreeBuilder::parse("<div style='position:absolute;left:0;top:0;width:20px;height:20px;background:red;color:green'></div>").document();
    let viewport = Rect {
        width: 40.0,
        height: 40.0,
        ..Default::default()
    };
    let mut resolver = StyleResolver::new();
    let mut environment = MediaEnvironment::default();
    environment.set_feature("forced-colors", "active");
    resolver.set_media_environment(environment);
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let mut pending = vec![document.clone()];
    let target = loop {
        let node = pending.pop().expect("styled div must exist");
        if node.tag_name().as_deref() == Some("div") {
            break node;
        }
        pending.extend(node.child_nodes());
    };
    let before = resolver.computed_style(&target).properties();
    let first = paint_layout(&layout, &mut resolver, viewport);
    let second = paint_layout(&layout, &mut resolver, viewport);
    assert_eq!(first.pixel(10, 10), Some(Color::rgb(255, 255, 255)));
    assert_eq!(second.pixel(10, 10), first.pixel(10, 10));
    assert_eq!(resolver.computed_style(&target).properties(), before);
}

#[test]
fn forced_color_adjust_validates_keywords_and_rejects_invalid_values() {
    let mut runtime = omoikane::js::JsRuntime::new().unwrap();
    for value in [
        "auto",
        "none",
        "preserve-parent-color",
        "NONE",
        "inherit",
        "initial",
        "unset",
        "revert",
        "revert-layer",
    ] {
        assert_eq!(
            runtime
                .eval(&format!("CSS.supports('forced-color-adjust', {value:?})"))
                .unwrap()
                .as_boolean(),
            Some(true),
            "{value}"
        );
    }
    for value in ["", "always", "auto none", "1", "none,auto"] {
        assert_eq!(
            runtime
                .eval(&format!("CSS.supports('forced-color-adjust', {value:?})"))
                .unwrap()
                .as_boolean(),
            Some(false),
            "{value}"
        );
    }
}

#[test]
fn native_palette_changes_and_disable_reach_used_pixels() {
    use omoikane::css::ForcedColorPalette;
    use omoikane::platform_media::PlatformMediaPreferences;
    let document = TreeBuilder::parse(
        "<div style='position:absolute;left:0;top:0;width:20px;height:20px;background:red'></div>",
    )
    .document();
    let viewport = Rect {
        width: 40.0,
        height: 40.0,
        ..Default::default()
    };
    let mut resolver = StyleResolver::new();
    let mut environment = MediaEnvironment::default();
    let mut palette = ForcedColorPalette::for_color_scheme(false);
    for rgb in [[11, 22, 33], [44, 55, 66]] {
        palette.canvas = rgb;
        PlatformMediaPreferences {
            forced_colors: Some(true),
            forced_color_palette: Some(palette),
            ..Default::default()
        }
        .apply_to(&mut environment);
        resolver.set_media_environment(environment.clone());
        let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
        assert_eq!(
            paint_layout(&layout, &mut resolver, viewport).pixel(10, 10),
            Some(Color::rgb(rgb[0], rgb[1], rgb[2]))
        );
    }
    PlatformMediaPreferences {
        forced_colors: Some(false),
        ..Default::default()
    }
    .apply_to(&mut environment);
    resolver.set_media_environment(environment);
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    assert_eq!(
        paint_layout(&layout, &mut resolver, viewport).pixel(10, 10),
        Some(Color::rgb(255, 0, 0))
    );
}

#[test]
fn forced_system_colors_are_supported_and_resolve_inside_opt_out() {
    use omoikane::css::ForcedColorPalette;
    let mut runtime = omoikane::js::JsRuntime::new().unwrap();
    assert_eq!(runtime.eval("['AccentColor','AccentColorText','ActiveText','ButtonBorder','ButtonFace','ButtonText','Canvas','CanvasText','Field','FieldText','GrayText','Highlight','HighlightText','LinkText','Mark','MarkText','SelectedItem','SelectedItemText','VisitedText'].every(name=>CSS.supports('color',name)) && !CSS.supports('color','CanvasTypo')").unwrap().as_boolean(),Some(true));
    let document = TreeBuilder::parse("<div style='position:absolute;left:0;top:0;width:20px;height:20px;background:Canvas;forced-color-adjust:none'></div><div style='position:absolute;left:20px;top:0;width:20px;height:20px;background:ButtonFace;forced-color-adjust:none'></div>").document();
    let viewport = Rect {
        width: 60.0,
        height: 40.0,
        ..Default::default()
    };
    let mut environment = MediaEnvironment::default();
    environment.set_feature("forced-colors", "active");
    let mut palette = ForcedColorPalette::for_color_scheme(false);
    palette.canvas = [11, 22, 33];
    palette.button_face = [44, 55, 66];
    environment.forced_color_palette = Some(palette);
    let mut resolver = StyleResolver::new();
    resolver.set_media_environment(environment);
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let pixels = paint_layout(&layout, &mut resolver, viewport);
    assert_eq!(pixels.pixel(10, 10), Some(Color::rgb(11, 22, 33)));
    assert_eq!(pixels.pixel(30, 10), Some(Color::rgb(44, 55, 66)));
}

#[test]
fn forced_gradient_suppression_is_computed_and_opt_out_preserves_it() {
    let document = TreeBuilder::parse("<div id='auto' style='position:absolute;left:0;top:0;width:20px;height:20px;background:red linear-gradient(red,red);box-shadow:0px 0px 2px red;text-shadow:0px 0px 2px red'></div><div id='none' style='position:absolute;left:20px;top:0;width:20px;height:20px;background:red linear-gradient(red,red);forced-color-adjust:none'></div>").document();
    let viewport = Rect {
        width: 60.0,
        height: 40.0,
        ..Default::default()
    };
    let mut resolver = StyleResolver::new();
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let normal = paint_layout(&layout, &mut resolver, viewport);
    assert_eq!(normal.pixel(10, 10), Some(Color::rgb(255, 0, 0)));
    let mut environment = MediaEnvironment::default();
    environment.set_feature("forced-colors", "active");
    resolver.set_media_environment(environment);
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let forced = paint_layout(&layout, &mut resolver, viewport);
    assert_eq!(forced.pixel(10, 10), Some(Color::rgb(255, 255, 255)));
    assert_eq!(forced.pixel(30, 10), Some(Color::rgb(255, 0, 0)));
    let mut pending = vec![document.clone()];
    let target = loop {
        let node = pending.pop().unwrap();
        if node.get_attribute("id").as_deref() == Some("auto") {
            break node;
        }
        pending.extend(node.child_nodes());
    };
    let style = resolver.computed_style(&target);
    for property in ["background-image", "box-shadow", "text-shadow"] {
        assert!(
            matches!(style.get(property), Some(omoikane::css::ComputedValue::Keyword(value)) if value == "none"),
            "{property}"
        );
    }
}

#[test]
fn explicit_system_background_is_preserved_with_auto_adjustment() {
    use omoikane::css::ForcedColorPalette;
    let document=TreeBuilder::parse("<div style='position:absolute;left:0;top:0;width:20px;height:20px;background:Highlight'></div><div style='position:absolute;left:20px;top:0;width:20px;height:20px;background:rgb(11,22,33)'></div>").document();
    let viewport = Rect {
        width: 60.0,
        height: 40.0,
        ..Default::default()
    };
    let mut environment = MediaEnvironment::default();
    environment.set_feature("forced-colors", "active");
    let mut palette = ForcedColorPalette::for_color_scheme(false);
    palette.highlight = [11, 22, 33];
    environment.forced_color_palette = Some(palette);
    let mut resolver = StyleResolver::new();
    resolver.set_media_environment(environment);
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let pixels = paint_layout(&layout, &mut resolver, viewport);
    assert_eq!(pixels.pixel(10, 10), Some(Color::rgb(11, 22, 33)));
    assert_eq!(pixels.pixel(30, 10), Some(Color::rgb(255, 255, 255)));
}

#[test]
fn inherited_system_color_reaches_current_color_backgrounds() {
    use omoikane::css::ForcedColorPalette;
    let document = TreeBuilder::parse("<div style='color:Highlight'><div style='color:inherit'><div style='position:absolute;left:0;top:0;width:20px;height:20px;background:currentColor'></div></div></div><div style='color:rgb(11,22,33)'><div style='position:absolute;left:20px;top:0;width:20px;height:20px;background:currentColor'></div></div>").document();
    let viewport = Rect {
        width: 60.0,
        height: 40.0,
        ..Default::default()
    };
    let mut environment = MediaEnvironment::default();
    environment.set_feature("forced-colors", "active");
    let mut palette = ForcedColorPalette::for_color_scheme(false);
    palette.highlight = [11, 22, 33];
    environment.forced_color_palette = Some(palette);
    let mut resolver = StyleResolver::new();
    resolver.set_media_environment(environment);
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let pixels = paint_layout(&layout, &mut resolver, viewport);
    assert_eq!(pixels.pixel(10, 10), Some(Color::rgb(11, 22, 33)));
    assert_eq!(pixels.pixel(30, 10), Some(Color::rgb(255, 255, 255)));
}

#[test]
fn author_background_is_forced_to_the_system_foreground_pair() {
    use omoikane::css::ForcedColorPalette;
    let document = TreeBuilder::parse("<div style='position:absolute;left:0;top:0;width:20px;height:20px;color:HighlightText;background:red'></div><div style='position:absolute;left:20px;top:0;width:20px;height:20px;color:Canvas;background:red'></div>").document();
    let viewport = Rect {
        width: 60.0,
        height: 40.0,
        ..Default::default()
    };
    let mut environment = MediaEnvironment::default();
    environment.set_feature("forced-colors", "active");
    let mut palette = ForcedColorPalette::for_color_scheme(false);
    palette.highlight = [11, 22, 33];
    environment.forced_color_palette = Some(palette);
    let mut resolver = StyleResolver::new();
    resolver.set_media_environment(environment);
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let pixels = paint_layout(&layout, &mut resolver, viewport);
    assert_eq!(pixels.pixel(10, 10), Some(Color::rgb(11, 22, 33)));
    assert_eq!(pixels.pixel(30, 10), Some(Color::rgb(0, 0, 0)));
}

#[test]
fn forced_viewport_background_uses_root_policy_and_canvas_palette() {
    use omoikane::css::ForcedColorPalette;
    let viewport = Rect {
        width: 60.0,
        height: 40.0,
        ..Default::default()
    };
    for (html_style, body_style, expected) in [
        ("background:red", "", Color::rgb(11, 22, 33)),
        ("", "background:red;height:1px", Color::rgb(11, 22, 33)),
        (
            "background:red;forced-color-adjust:none",
            "",
            Color::rgb(255, 0, 0),
        ),
        (
            "",
            "background:red;height:1px;forced-color-adjust:none",
            Color::rgb(11, 22, 33),
        ),
        ("", "", Color::rgb(11, 22, 33)),
        (
            "forced-color-adjust:none",
            "background:red;height:1px",
            Color::rgb(255, 0, 0),
        ),
        ("background:Highlight", "", Color::rgb(44, 55, 66)),
    ] {
        let document = TreeBuilder::parse(&format!(
            "<html style='{html_style}'><body style='margin:0;{body_style}'></body></html>"
        ))
        .document();
        let mut environment = MediaEnvironment::default();
        environment.set_feature("forced-colors", "active");
        let mut palette = ForcedColorPalette::for_color_scheme(false);
        palette.canvas = [11, 22, 33];
        palette.highlight = [44, 55, 66];
        environment.forced_color_palette = Some(palette);
        let mut resolver = StyleResolver::new();
        resolver.set_media_environment(environment);
        let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
        let pixels = paint_layout(&layout, &mut resolver, viewport);
        assert_eq!(
            pixels.pixel(50, 30),
            Some(expected),
            "html={html_style}, body={body_style}"
        );
    }
}

#[test]
fn propagated_body_background_is_painted_once() {
    let document = TreeBuilder::parse(
        "<html><body style='margin:0;height:20px;background:rgba(255,0,0,0.5)'></body></html>",
    )
    .document();
    let viewport = Rect {
        width: 60.0,
        height: 40.0,
        ..Default::default()
    };
    let mut resolver = StyleResolver::new();
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let pixels = paint_layout(&layout, &mut resolver, viewport);
    let expected = Color {
        r: 255,
        g: 0,
        b: 0,
        a: 128,
    };
    assert_eq!(
        pixels.pixel(50, 30),
        Some(expected),
        "the body background must cover the canvas"
    );
    assert_eq!(
        pixels.pixel(10, 10),
        Some(expected),
        "the propagated background must not be painted a second time on the body"
    );
}

#[test]
fn propagated_root_background_is_painted_once() {
    let document = TreeBuilder::parse(
        "<html style='background:rgba(255,0,0,0.5)'><body style='margin:0;height:20px'></body></html>",
    ).document();
    let viewport = Rect {
        width: 60.0,
        height: 40.0,
        ..Default::default()
    };
    let mut resolver = StyleResolver::new();
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let pixels = paint_layout(&layout, &mut resolver, viewport);
    assert_eq!(
        pixels.pixel(10, 10),
        Some(Color {
            r: 255,
            g: 0,
            b: 0,
            a: 128
        })
    );
}

#[test]
fn viewport_propagation_preserves_source_current_color_and_computed_style() {
    use omoikane::css::ForcedColorPalette;
    let document = TreeBuilder::parse(
        "<html style='color:blue;forced-color-adjust:none'><body id='body' style='margin:0;height:20px;color:red;background:currentColor'></body></html>",
    ).document();
    let mut pending = vec![document.clone()];
    let body = loop {
        let node = pending.pop().expect("body must exist");
        if node.tag_name().as_deref() == Some("body") {
            break node;
        }
        pending.extend(node.child_nodes());
    };
    let viewport = Rect {
        width: 60.0,
        height: 40.0,
        ..Default::default()
    };
    let mut resolver = StyleResolver::new();
    let mut environment = MediaEnvironment::default();
    environment.set_feature("forced-colors", "active");
    let mut palette = ForcedColorPalette::for_color_scheme(false);
    palette.canvas = [11, 22, 33];
    environment.forced_color_palette = Some(palette);
    resolver.set_media_environment(environment);
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    let original = resolver.computed_style(&body);
    let pixels = paint_layout(&layout, &mut resolver, viewport);
    assert_eq!(pixels.pixel(50, 30), Some(Color::rgb(255, 0, 0)));
    assert_eq!(pixels.pixel(10, 10), Some(Color::rgb(255, 0, 0)));
    assert_eq!(resolver.computed_style(&body), original);
    resolver.set_media_environment(MediaEnvironment::default());
    let layout = layout_tree(&document, &mut resolver, viewport).unwrap();
    assert_eq!(
        paint_layout(&layout, &mut resolver, viewport).pixel(50, 30),
        Some(Color::rgb(255, 0, 0))
    );
}
