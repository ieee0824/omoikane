use super::*;

fn eval_text(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .unwrap()
        .as_string()
        .expect("string result")
        .to_std_string_escaped()
}

fn has_layout(runtime: &JsRuntime) -> bool {
    runtime.host_state.borrow().layout_root.is_some()
}

#[test]
fn computed_style_reads_lay_out_only_for_resolved_sizes() {
    let document = crate::html::TreeBuilder::parse(
        "<style>#target { color: red; width: 40px; }</style><div id='target'></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval("globalThis.target = document.getElementById('target'); target.style.color = 'blue';")
        .unwrap();
    assert!(
        !has_layout(&runtime),
        "an inline style change drops the layout"
    );

    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(target).color"),
        "rgb(0, 0, 255)"
    );
    assert_eq!(
        eval_text(&mut runtime, "String('color' in getComputedStyle(target))"),
        "true"
    );
    assert!(
        !has_layout(&runtime),
        "getComputedStyle and non-size reads must not lay out the document"
    );

    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(target).width"),
        "40px"
    );
    assert!(has_layout(&runtime), "a width read resolves the used size");
    runtime.eval("target.style.color = 'green'").unwrap();
    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(target).inlineSize"),
        "40px"
    );
    assert!(
        has_layout(&runtime),
        "inline-size aliases the resolved width"
    );
}

#[test]
fn container_dependent_computed_values_use_current_layout() {
    let document = crate::html::TreeBuilder::parse(
        r#"<style>
            #shell { width: 600px; container-type: inline-size; container-name: shell; }
            @container shell (inline-size >= 400px) { #item { color: rgb(0, 128, 0); } }
            #tile { color: color(srgb calc(0.5 + (sign(2cqw - 10px) * 0.1)) 0 0); }
        </style><section id="shell"><p id="item"></p><p id="tile"></p></section>"#,
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime
        .eval(
            "globalThis.shell = document.getElementById('shell');
             globalThis.item = document.getElementById('item');
             globalThis.tile = document.getElementById('tile');",
        )
        .unwrap();
    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(item).color"),
        "rgb(0, 128, 0)"
    );
    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(tile).color"),
        "color(srgb 0.6 0 0)"
    );

    runtime.eval("shell.style.width = '300px'").unwrap();
    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(item).color"),
        "rgb(0, 0, 0)",
        "a container query must be re-evaluated against the new container size"
    );
    assert_eq!(
        eval_text(&mut runtime, "getComputedStyle(tile).color"),
        "color(srgb 0.4 0 0)",
        "container-relative colors must use the new container size"
    );
}

#[test]
fn named_logical_cssom_reads_match_full_snapshots_after_flow_changes() {
    let document = crate::html::TreeBuilder::parse(
        "<div id='target' style='width:80px;height:40px;color:rgb(10,20,30);\
         margin:1px 2px 3px 4px;padding:5px 6px 7px 8px;\
         border-style:solid;border-width:1px 2px 3px 4px;min-width:auto;min-height:auto'></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let result = eval_text(
        &mut runtime,
        r#"
        (() => {
            const element = document.getElementById('target');
            const style = getComputedStyle(element);
            for (const [writingMode, direction] of [
                ['horizontal-tb', 'ltr'], ['horizontal-tb', 'rtl'],
                ['vertical-rl', 'ltr'], ['vertical-lr', 'rtl']
            ]) {
                element.style.writingMode = writingMode;
                element.style.direction = direction;
                const snapshot = new Map(style.cssText.split(';').filter(Boolean).map(row => {
                    const colon = row.indexOf(':');
                    return [row.slice(0, colon).trim(), row.slice(colon + 1).trim()];
                }));
                for (const [name, value] of snapshot) {
                    if (style.getPropertyValue(name) !== value) {
                        throw new Error(writingMode + '/' + direction + ': ' + name);
                    }
                }
                if (style.color !== 'rgb(10, 20, 30)' || style.width !== '80px') {
                    throw new Error('physical value changed');
                }
                const inlineSize = writingMode === 'horizontal-tb' ? '80px' : '40px';
                if (style.inlineSize !== inlineSize || style.minInlineSize !== '0px'
                    || style.borderInlineStartColor !== 'rgb(10, 20, 30)') {
                    throw new Error('logical value changed');
                }
            }
            const shell = document.createElement('section');
            shell.style.display = 'flex';
            document.body.appendChild(shell);
            shell.appendChild(element);
            if (style.minInlineSize !== 'auto') throw new Error('flex auto minimum changed');
            return 'ok';
        })()
    "#,
    );
    assert_eq!(result, "ok");
}

fn used_size_fixture() -> (NodeHandle, LayoutBox) {
    let document = crate::html::TreeBuilder::parse(
        r#"<style>
        #outer { transform: rotate(15deg) scale(2); width: 90px; }
        #block { width: 40px; height: 25px; margin: 7px; padding: 3px; border: 2px solid; }
        #inline { padding: 2px; border: 1px solid; }
        #hidden { display: none; }
        #pseudo::before { content: 'before'; display: block; width: 13px; height: 17px; }
        </style><div id="outer"><div id="block"></div>
        <span id="inline">one two three four five six seven eight nine ten</span>
        <input id="control" value="hello"><input id="icon" type="checkbox">
        <img id="image" width="23" height="19">
        <div id="hidden"></div><div id="pseudo"></div></div>"#,
    )
    .document();
    let runtime = JsRuntime::with_document(document.clone()).unwrap();
    let mut state = runtime.host_state.borrow_mut();
    state.ensure_layout();
    (document, state.layout_root.as_ref().unwrap().clone())
}

#[test]
fn used_size_geometry_search_preserves_box_edges_and_node_identity() {
    let (document, root) = used_size_fixture();
    let block = document.query_selector("#block").unwrap();
    let size = layout_used_size(&root, &block, None).unwrap();
    assert_eq!((size.content_width, size.content_height), (40.0, 25.0));
    assert_eq!((size.border_width, size.border_height), (50.0, 35.0));
    let (margin, padding) = size.edges.unwrap();
    assert_eq!((margin.left, margin.top), (7.0, 7.0));
    assert_eq!((padding.left, padding.top), (3.0, 3.0));
    assert!(!size.non_replaced_inline);
    assert_eq!(
        layout_used_size(&root, &NodeHandle::element("div"), None),
        None
    );
    assert_eq!(
        layout_used_size(&root, &document.query_selector("#hidden").unwrap(), None),
        None
    );
    let pseudo = document.query_selector("#pseudo").unwrap();
    let before = layout_used_size(&root, &pseudo, Some(PseudoElement::Before)).unwrap();
    assert_eq!((before.content_width, before.content_height), (13.0, 17.0));
    assert_eq!(
        layout_used_size(&root, &pseudo, Some(PseudoElement::After)),
        None
    );
}

#[test]
fn used_size_geometry_search_matches_transformed_search_selection() {
    let (document, root) = used_size_fixture();
    for id in [
        "block", "inline", "control", "icon", "image", "hidden", "pseudo",
    ] {
        let node = document.query_selector(&format!("#{id}")).unwrap();
        let mut previous_fragments = Vec::new();
        let previous_box = find_layout_box_with_transform(
            &root,
            &node,
            AffineTransform::identity(),
            &mut previous_fragments,
        );
        let mut first_fragment = None;
        let actual_box = find_used_size_box(&root, &node, &mut first_fragment);
        assert_eq!(actual_box, previous_box.map(|(layout, _)| layout), "{id}");
        if previous_box.is_some() {
            continue;
        }
        assert_eq!(
            first_fragment.map(|fragment| fragment.rect),
            previous_fragments.first().map(|fragment| fragment.rect),
            "{id}"
        );
        if let Some(previous) = previous_fragments.first() {
            let actual = layout_used_size(&root, &node, None).unwrap();
            let padding = edge_sizes(&previous.style, "padding");
            let border = edge_sizes(&previous.style, "border");
            assert_eq!(actual.non_replaced_inline, previous.non_replaced, "{id}");
            assert_eq!(actual.border_width, previous.rect.width, "{id}");
            assert_eq!(actual.border_height, previous.rect.height, "{id}");
            assert_eq!(
                actual.content_width,
                (previous.rect.width - padding.left - padding.right - border.left - border.right)
                    .max(0.0),
                "{id}"
            );
            assert_eq!(
                actual.content_height,
                (previous.rect.height - padding.top - padding.bottom - border.top - border.bottom)
                    .max(0.0),
                "{id}"
            );
        }
    }
}

#[test]
fn used_size_geometry_search_keeps_first_split_fragment_but_prefers_later_box() {
    let (document, mut root) = used_size_fixture();
    let inline = document.query_selector("#inline").unwrap();
    let block = document.query_selector("#block").unwrap();
    let mut fragments = Vec::new();
    assert!(
        find_layout_box_with_transform(&root, &inline, AffineTransform::identity(), &mut fragments)
            .is_none()
    );
    assert!(
        fragments.len() > 1,
        "fixture must wrap into multiple inline boxes"
    );
    let first = fragments[0].rect;
    let size = layout_used_size(&root, &inline, None).unwrap();
    assert!(size.non_replaced_inline);
    assert_eq!(
        (size.border_width, size.border_height),
        (first.width, first.height)
    );

    // Append an actual box for the same node after the earlier inline fragments.
    // Returning immediately on a fragment would silently change this precedence.
    let mut unused = Vec::new();
    let mut later_box =
        find_layout_box_with_transform(&root, &block, AffineTransform::identity(), &mut unused)
            .unwrap()
            .0
            .clone();
    later_box.node = inline.clone();
    later_box.dimensions.content.width = 123.0;
    root.children.push(later_box);
    let size = layout_used_size(&root, &inline, None).unwrap();
    assert!(!size.non_replaced_inline);
    assert_eq!(size.content_width, 123.0);
    assert!(size.edges.is_some());
}
