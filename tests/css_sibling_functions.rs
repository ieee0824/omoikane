use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn aligned_auto_grid_items_keep_the_grid_area_as_their_percentage_basis() {
    let document = TreeBuilder::parse(
        "<style>#parent{display:grid;width:100px;grid-template-columns:50px}\
         #first{display:none}#target{justify-self:start;height:1px;\
         margin-left:abs(calc(sibling-index()*10% - 30px));\
         padding-left:mod(calc(sibling-index()*10% + 5px),15px)}</style>\
         <div id='parent'><span id='first'></span><div id='target'></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');const read=()=>{\
         const s=getComputedStyle(e);return [s.marginLeft,s.paddingLeft]};\
         const before=read();document.getElementById('first').remove();\
         return JSON.stringify([before,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, r#"[["20px","0px"],["25px","10px"]]"#);
}

#[test]
fn inline_tree_counting_edges_keep_their_computed_percentage_math() {
    let document = TreeBuilder::parse(
        "<style>#parent{width:100px}#target{display:inline;\
         margin-left:abs(calc(sibling-index()*10% - 30px));\
         padding-left:hypot(calc(sibling-index()*3px),calc(sibling-index()*4%))}</style>\
         <div id='parent'><span id='first'></span><span id='target'>x</span></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');const read=()=>{\
         const s=getComputedStyle(e);return [s.marginLeft,s.paddingLeft]};\
         const before=read();document.getElementById('first').remove();\
         return JSON.stringify([before,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["abs(20% - 30px)","hypot(6px, 8%)"],["abs(10% - 30px)","hypot(3px, 4%)"]]"#
    );
}

#[test]
fn flex_and_grid_tree_counting_edges_use_their_containing_block_basis() {
    let document = TreeBuilder::parse(
        "<style>.parent{width:100px}.first{display:none}.target{width:20px;flex:none;\
         margin-left:abs(calc(sibling-index()*10% - 30px));\
         padding-left:mod(calc(sibling-index()*10% + 5px),15px)}\
         #flex{display:flex}#grid{display:grid;grid-template-columns:50px}</style>\
         <div id='flex' class='parent'><span class='first' id='flex-first'></span>\
         <div id='flex-target' class='target'></div></div>\
         <div id='grid' class='parent'><span class='first' id='grid-first'></span>\
         <div id='grid-target' class='target'></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const read=()=>['flex','grid'].map(id=>{\
             const s=getComputedStyle(document.getElementById(id+'-target'));\
             return [s.marginLeft,s.paddingLeft]});const before=read();\
             for(const id of ['flex','grid'])document.getElementById(id+'-first').remove();\
             return JSON.stringify([before,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[[["10px","10px"],["20px","0px"]],[["20px","0px"],["25px","10px"]]]"#
    );
}

#[test]
fn registered_percentage_functions_compute_without_a_layout_basis() {
    let document = TreeBuilder::parse(
        "<style>@property --norm{syntax:'<percentage>';inherits:true;initial-value:0%}\
         @property --absolute{syntax:'<percentage>';inherits:true;initial-value:0%}\
         @property --modulo{syntax:'<percentage>';inherits:true;initial-value:0%}\
         #parent{width:100px}#target{\
         --norm:hypot(calc(sibling-index()*3%),calc(sibling-index()*4%));\
         --absolute:abs(calc(sibling-index()*-1%));\
         --modulo:mod(calc(sibling-index()*10%),15%);width:var(--norm)}</style>\
         <div id='parent'><span id='first'></span><div id='target'></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');const read=()=>{\
             const s=getComputedStyle(e);return [s.getPropertyValue('--norm').trim(),\
             s.width,s.getPropertyValue('--absolute').trim(),s.getPropertyValue('--modulo').trim()]};\
             const before=read();document.getElementById('first').remove();\
             return JSON.stringify([before,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["10%","10px","2%","5%"],["5%","5px","1%","10%"]]"#
    );
}

#[test]
fn tree_counting_numeric_functions_defer_mixed_percentages_to_layout() {
    let document = TreeBuilder::parse(
        "<style>#parent{width:100px}#target{\
         width:hypot(calc(sibling-index()*3px),calc(sibling-index()*4%));\
         margin-left:abs(calc(sibling-index()*10% - 30px));\
         padding-left:mod(calc(sibling-index()*10% + 5px),15px);\
         padding-right:rem(calc(sibling-index()*10% + 5px),15px)}</style>\
         <div id='parent'><span id='first'></span><div id='target'></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');\
             const read=()=>{const s=getComputedStyle(e);return [\
             s.width,s.marginLeft,s.paddingLeft,s.paddingRight]};\
             const before=read();document.getElementById('first').remove();\
             return JSON.stringify([before,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["10px","10px","10px","10px"],["5px","20px","0px","0px"]]"#
    );
}

#[test]
fn font_tree_counting_grammar_rejects_arguments_and_dimension_mismatches() {
    let mut runtime = JsRuntime::new().unwrap();
    let actual = runtime
        .eval(
            "JSON.stringify([\
             ['font-style','oblique calc(sibling-index()*1deg)'],\
             ['font-style','oblique sibling-index()'],\
             ['font-style','oblique calc(sibling-index(1)*1deg)'],\
             ['font-variation-settings','\"wght\" sibling-index()'],\
             ['font-variation-settings','\"wght\" sibling-index(1)'],\
             ['font-variation-settings','\"wght\" calc(sibling-index()*1px)']\
             ].map(([property,value])=>CSS.supports(property,value)))",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, "[true,false,false,true,false,false]");
}

#[test]
fn registered_angles_and_oblique_fonts_compute_inverse_trig_tree_counts() {
    let document = TreeBuilder::parse(
        "<style>@property --angle{syntax:'<angle>';inherits:true;initial-value:0deg}\
         #target{--angle:atan2(sibling-index(),0);rotate:var(--angle);\
         font-style:oblique atan2(sibling-index(),0)}</style>\
         <div><span id='first'></span><div id='target'></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');\
             const read=()=>{const s=getComputedStyle(e);return [\
             s.getPropertyValue('--angle').trim(),s.rotate,s.fontStyle]};\
             const before=read();document.getElementById('first').remove();\
             return JSON.stringify([before,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["90deg","90deg","oblique 90deg"],["90deg","90deg","oblique 90deg"]]"#
    );
}

#[test]
fn ordinary_font_variation_counts_compute_and_inherit_after_dom_changes() {
    let document = TreeBuilder::parse(
        "<style>#parent{font-variation-settings:'wght' sibling-index()}</style>\
         <div><span id='first'></span><div id='parent'><div id='child'></div></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const read=()=>['parent','child'].map(id=>\
             getComputedStyle(document.getElementById(id)).fontVariationSettings);\
             const before=read();document.getElementById('first').remove();\
             return JSON.stringify([before,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["\"wght\" 2","\"wght\" 2"],["\"wght\" 1","\"wght\" 1"]]"#
    );
}

#[test]
fn registered_alias_inherits_the_parents_computed_integer() {
    let document = TreeBuilder::parse(
        "<style>@property --index{syntax:'<integer>';inherits:true;initial-value:0}\
         #target{--index:sibling-index();--alias:var(--index)}\
         #child{flex-grow:var(--alias);flex-shrink:var(--raw)}\
         #target{--raw:sibling-index()}</style><div><span></span>\
         <div id='target'><span id='child'></span></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const s=getComputedStyle(document.getElementById('child'));\
         return JSON.stringify([s.flexGrow,s.flexShrink,s.getPropertyValue('--alias')]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, r#"["2","1","2"]"#);
}

#[test]
fn finite_active_keyframes_recompute_tree_counting_values_after_dom_changes() {
    let document = TreeBuilder::parse(
        "<style>@keyframes count{from{width:calc(sibling-index() * 10px);\
         opacity:calc(sibling-index() * .1)}to{width:100px;opacity:1}}\
         #target{animation:count 1000s step-end}</style>\
         <div><span id='first'></span><span></span><div id='target'></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');\
         const read=()=>{const s=getComputedStyle(e);return [s.width,Number(s.opacity)]};\
         const before=read();document.getElementById('first').remove();\
         return JSON.stringify([before,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, r#"[["30px",0.3],["20px",0.2]]"#);
}

#[test]
fn nested_numeric_comparisons_feed_length_percentage_expressions() {
    let document = TreeBuilder::parse(
        "<style>#parent{width:200px;height:100px}#target{\
         width:calc(min(sibling-index(),3) * 10% + 5px);\
         height:calc(clamp(1,sibling-count(),4) * 5% + 3px);\
         flex-grow:min(sibling-index(),3)}</style>\
         <div id='parent'><span></span><div id='target'></div><span></span></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');\
         const read=()=>{const s=getComputedStyle(e);return [s.width,s.height,s.flexGrow]};\
         const before=read();e.parentNode.insertBefore(document.createElement('span'),e);\
         return JSON.stringify([before,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, r#"[["45px","18px","2"],["65px","23px","3"]]"#);
}

#[test]
fn registered_values_compute_before_inheritance_but_unregistered_tokens_remain() {
    let document = TreeBuilder::parse(
        "<style>@property --index{syntax:'<integer>';inherits:true;initial-value:0}\
         @property --size{syntax:'<length>';inherits:true;initial-value:0px}\
         #target{--index:sibling-index();--size:calc(sibling-count() * 5px);\
         --raw:sibling-index()}</style><div><span></span>\
         <div id='target'><span id='child'></span></div><span></span></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');\
         const read=id=>{const s=getComputedStyle(document.getElementById(id));\
         return ['--index','--size','--raw'].map(n=>s.getPropertyValue(n))};\
         const before=[read('target'),read('child')];\
         e.parentNode.insertBefore(document.createElement('span'),e);\
         return JSON.stringify([before,[read('target'),read('child')]]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[[["2","15px","sibling-index()"],["2","15px","sibling-index()"]],[["3","20px","sibling-index()"],["3","20px","sibling-index()"]]]"#
    );
}

#[test]
fn sibling_functions_compute_integer_values_and_update_after_dom_mutation() {
    let document = TreeBuilder::parse(
        "<style>#target {width:calc(sibling-index() * 10px);\
         height:calc(sibling-count() * 5px);flex-grow:sibling-index();\
         --count:sibling-count();column-count:var(--count)}</style>\
         <div id='parent'><span></span>text<!--comment--><span id='target'></span>\
         <span></span></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => { const e=document.getElementById('target');\
             const read=()=>{const s=getComputedStyle(e);\
             return [s.width,s.height,s.flexGrow,s.columnCount];};\
             const before=read();const inserted=document.createElement('span');\
             e.parentNode.insertBefore(inserted,e);const after=read();\
             inserted.remove();return JSON.stringify([before,after,read()]); })()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["20px","15px","2","3"],["30px","20px","3","4"],["20px","15px","2","3"]]"#
    );
}

#[test]
fn sibling_functions_validate_arguments_and_numeric_types() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(
        runtime
            .eval(
                "CSS.supports('flex-grow','sibling-index()') &&\
             CSS.supports('column-count','sibling-count()') &&\
             CSS.supports('flex-grow','sibling-index( )') &&\
             CSS.supports('width','calc(sibling-index() * 1px)') &&\
             !CSS.supports('width','sibling-index()') &&\
             !CSS.supports('width','calc(0 * sibling-index())') &&\
             !CSS.supports('width','calc(0)') &&\
             CSS.supports('width','0') &&\
             !CSS.supports('flex-grow','sibling-index(1)') &&\
             !CSS.supports('column-count','sibling-count(of div)')",
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn registered_dimensions_preserve_units_through_inherited_aliases() {
    let document = TreeBuilder::parse(
        "<style>@property --time{syntax:'<time>';inherits:true;initial-value:0s}\
         @property --angle{syntax:'<angle>';inherits:true;initial-value:0deg}\
         #parent{--time:calc(2s * sibling-index());--angle:calc(30deg * sibling-index());\
         --delay:var(--time);--rotation:var(--angle)}\
         #child{transition-duration:var(--delay);rotate:var(--rotation)}</style>\
         <div><span></span><div id='parent'><div id='child'></div></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const p=getComputedStyle(document.getElementById('parent'));\
         const c=getComputedStyle(document.getElementById('child'));\
         return JSON.stringify([p.getPropertyValue('--time'),p.getPropertyValue('--angle'),\
         c.transitionDuration,c.rotate]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, r#"["4s","60deg","4s","60deg"]"#);
}

#[test]
fn keyframes_evaluate_inherited_unregistered_aliases_for_the_current_element() {
    let document = TreeBuilder::parse(
        "<style>@keyframes count{from{width:var(--amount)}to{width:100px}}\
         #parent{--amount:calc(sibling-index() * 10px)}\
         #target{animation:count 1000s step-end}</style>\
         <div id='parent'><span id='first'></span><span></span><div id='target'></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const e=document.getElementById('target');const read=()=>getComputedStyle(e).width;\
         const before=read();document.getElementById('first').remove();\
         return JSON.stringify([before,read()]);})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(actual, r#"["30px","20px"]"#);
}

#[test]
fn universal_registration_preserves_tree_counting_tokens_through_inheritance() {
    let document = TreeBuilder::parse(
        "<style>@property --raw{syntax:'*';inherits:true;initial-value:0}\
         #parent{--raw:sibling-index();--alias:var(--raw)}\
         #child{flex-grow:var(--alias)}</style>\
         <div><span></span><div id='parent'><div id='child'></div></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const p=getComputedStyle(document.getElementById('parent'));\
         const c=getComputedStyle(document.getElementById('child'));\
         return JSON.stringify([p.getPropertyValue('--raw'),c.getPropertyValue('--raw'),\
         c.getPropertyValue('--alias'),c.flexGrow]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"["sibling-index()","sibling-index()","sibling-index()","1"]"#
    );
}

#[test]
fn inherited_aliases_keep_their_definition_scope_inside_shadow_keyframes() {
    let document = TreeBuilder::parse(
        "<style>#host{--amount:calc(sibling-index() * 10px)}</style>\
         <div><span></span><div id='host'></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const host=document.getElementById('host');\
         const root=host.attachShadow({mode:'open'});\
         root.innerHTML='<style>@keyframes count{from{width:var(--amount)}to{width:100px}}\
         #target{animation:count 1000s step-end}</style><div id=target></div>';\
         return getComputedStyle(root.getElementById('target')).width;})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, "0px");
}

#[test]
fn cssom_canonicalizes_tree_counting_numeric_values_on_each_assignment_path() {
    let document = TreeBuilder::parse("<div id='target'></div>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const e=document.getElementById('target');\
         e.style.zIndex='calc(sibling-index())';\
         e.style.setProperty('animation-duration','calc(100ms * sibling-count( ))');\
         const direct=[e.style.zIndex,e.style.animationDuration];\
         e.style.cssText='z-index:calc(sibling-count());animation-duration:calc(250ms * sibling-index())';\
         return JSON.stringify([direct,[e.style.zIndex,e.style.animationDuration]]);})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["sibling-index()","calc(0.1s * sibling-count())"],["sibling-count()","calc(0.25s * sibling-index())"]]"#
    );
}

#[test]
fn animated_registered_counts_recompose_dependent_declarations_after_dom_changes() {
    let document = TreeBuilder::parse(
        "<style>@property --size{syntax:'<length>';inherits:true;initial-value:0px}\
         @keyframes count{from{--size:calc(sibling-index() * 10px)}to{--size:100px}}\
         #target{animation:count 1000s step-end;width:var(--size);--alias:var(--size);height:var(--alias)}\
         #child{width:var(--size)}</style>\
         <div><span id='first'></span><span></span><div id='target'><div id='child'></div></div></div>",
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const e=document.getElementById('target');\
         const read=()=>{const s=getComputedStyle(e);\
         return [s.getPropertyValue('--size'),s.width,s.height,getComputedStyle(document.getElementById('child')).width]};\
         const before=read();document.getElementById('first').remove();\
         return JSON.stringify([before,read()]);})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["30px","30px","30px","30px"],["20px","20px","20px","20px"]]"#
    );
}

#[test]
fn untyped_animated_counts_keep_tokens_and_recompose_at_each_element() {
    let mut failures = Vec::new();
    for registration in [
        "",
        "@property --raw{syntax:'*';inherits:true;initial-value:0}",
    ] {
        let source = format!(
            "<style>{registration}\
             @keyframes count{{from{{--raw:sibling-index()}}to{{--raw:sibling-count()}}}}\
             #target{{animation:count 1000s step-end;flex-grow:var(--raw)}}\
             #child{{flex-grow:var(--raw)}}</style>\
             <div><span id='first'></span><div id='target'><div id='child'></div></div><span></span></div>"
        );
        let document = TreeBuilder::parse(&source).document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        let actual = runtime.eval(
            "(() => {const e=document.getElementById('target'),c=document.getElementById('child');\
             const read=()=>{const p=getComputedStyle(e),s=getComputedStyle(c);\
             return [p.getPropertyValue('--raw'),p.flexGrow,s.getPropertyValue('--raw'),s.flexGrow]};\
             const before=read();document.getElementById('first').remove();\
             return JSON.stringify([before,read()]);})()"
        ).unwrap().as_string().unwrap().to_std_string_escaped();
        let expected = r#"[["sibling-index()","2","sibling-index()","1"],["sibling-index()","1","sibling-index()","1"]]"#;
        if actual != expected {
            failures.push((registration, actual));
        }
    }
    assert!(
        failures.is_empty(),
        "untyped animation mismatches: {failures:?}"
    );
}

#[test]
fn important_custom_values_win_over_tree_counting_keyframes() {
    let mut failures = Vec::new();
    for registration in [
        "",
        "@property --amount{syntax:'*';inherits:true;initial-value:0}",
        "@property --amount{syntax:'<number>';inherits:true;initial-value:0}",
    ] {
        let source = format!(
            "<style>{registration}\
             @keyframes count{{from{{--amount:sibling-index()}}to{{--amount:13}}}}\
             #target{{--amount:7!important;animation:count 1000s step-end;flex-grow:var(--amount)}}\
             #child{{flex-grow:var(--amount)}}</style>\
             <div><span></span><div id='target'><div id='child'></div></div></div>"
        );
        let document = TreeBuilder::parse(&source).document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        let actual = runtime
            .eval(
                "(() => {const p=getComputedStyle(document.getElementById('target'));\
             const c=getComputedStyle(document.getElementById('child'));\
             return JSON.stringify([p.getPropertyValue('--amount'),p.flexGrow,c.flexGrow]);})()",
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        if actual != r#"["7","7","7"]"# {
            failures.push((registration, actual));
        }
    }
    assert!(
        failures.is_empty(),
        "important custom animation mismatches: {failures:?}"
    );
}

#[test]
fn fallback_counts_recompute_after_moves_within_and_between_parents() {
    let document = TreeBuilder::parse(
        "<style>#target{width:var(--missing,calc(sibling-index() * 10px));\
         flex-grow:var(--absent,sibling-count())}</style>\
         <div id='parent'>text<span></span><!--comment--><span></span><div id='target'></div><span></span></div>\
         <div id='other'><span></span></div>"
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target'),p=e.parentNode;\
         const read=()=>{const s=getComputedStyle(e);return [s.width,s.flexGrow]};\
         const before=read();p.insertBefore(e,p.firstChild);const reordered=read();\
         document.getElementById('other').appendChild(e);\
         return JSON.stringify([before,reordered,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, r#"[["30px","4"],["10px","4"],["20px","2"]]"#);
}

#[test]
fn untyped_animated_aliases_retain_document_scope_when_inherited_into_shadow() {
    let document = TreeBuilder::parse(
        "<style>@property --raw{syntax:'*';inherits:true;initial-value:0}\
         @keyframes count{from{--raw:sibling-index()}to{--raw:sibling-count()}}\
         #host{animation:count 1000s step-end;flex-grow:var(--raw)}</style>\
         <div><span id='first'></span><div id='host'></div><span></span></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const host=document.getElementById('host');\
         const root=host.attachShadow({mode:'open'});\
         root.innerHTML='<style>#inner{--alias:var(--raw);flex-grow:var(--alias)}</style><div id=inner></div>';\
         const inner=root.getElementById('inner');\
         const read=()=>{const p=getComputedStyle(host),c=getComputedStyle(inner);\
         return [p.getPropertyValue('--raw'),p.flexGrow,c.getPropertyValue('--alias'),c.flexGrow]};\
         const before=read();document.getElementById('first').remove();\
         return JSON.stringify([before,read()]);})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    // CSS Shadow §4.2 retains the reference root during inheritance. Values 5
    // tree counting returns zero when that root cannot match a descendant tree.
    assert_eq!(
        actual,
        r#"[["sibling-index()","2","sibling-index()","0"],["sibling-index()","1","sibling-index()","0"]]"#
    );
}

#[test]
fn nested_markers_count_the_ultimate_element_and_inherit_their_originating_pseudo() {
    let document = TreeBuilder::parse(
        "<style>#target::before{content:'before';display:list-item;font-style:italic;--total:sibling-count()}\
         #target::before::marker{font-weight:calc(sibling-index() * 2);font-size:calc(var(--total) * 2px)}\
         #target::after{content:'after';display:list-item;font-style:oblique}\
         #target::after::marker{font-weight:calc(sibling-index() * 3);font-size:calc(sibling-count() * 3px)}</style>\
         <div><span id='first'></span><div id='target'></div><span></span></div>"
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const e=document.getElementById('target');\
         const read=()=>['::before::marker','::after::marker'].map(p=>{const s=getComputedStyle(e,p);\
         return [s.fontWeight,s.fontSize,s.fontStyle]});\
         const before=read();document.getElementById('first').remove();\
         return JSON.stringify([before,read(),getComputedStyle(e,'::marker::before').getPropertyValue('font-size')]);})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[[["4","6px","italic"],["6","9px","oblique"]],[["2","4px","italic"],["3","6px","oblique"]],""]"#
    );
}

#[test]
fn specified_angles_fold_fixed_factors_without_evaluating_tree_counts() {
    let document = TreeBuilder::parse("<div id='target'></div>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');\
         const read=value=>{e.style.rotate=value;return e.style.rotate};\
         return JSON.stringify([read('calc(sibling-index() * 2rad * pi)'),\
         read('calc(sibling-count() * 1rad * pi)'),\
         read('calc(sibling-index() * sin(30deg) * 1turn)')]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"["calc(360deg * sibling-index())","calc(180deg * sibling-count())","calc(180deg * sibling-index())"]"#
    );
}

#[test]
fn cssom_defers_variable_substitution_without_losing_tree_counting_tokens() {
    let document = TreeBuilder::parse(
        "<style>#target{rotate:45deg}</style><div><span></span><div id='target' style='--angle:10deg'></div></div>"
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');e.style.rotate='none';\
         return JSON.stringify(['calc(sibling-index() * var(--angle))',\
         'var(--fallback, calc(sibling-index() * 1rad * pi))',\
         'calc(sibling-index(1) * var(--angle))'].map(v=>{e.style.rotate=v;\
         return [CSS.supports('rotate',v),e.style.rotate,getComputedStyle(e).rotate]}));})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[[true,"calc(sibling-index() * var(--angle))","20deg"],[true,"var(--fallback, calc(sibling-index() * 1rad * pi))","360deg"],[true,"calc(sibling-index(1) * var(--angle))","none"]]"#
    );
}

#[test]
fn invalid_variable_tree_counting_types_compute_as_unset_after_winning_the_cascade() {
    let document = TreeBuilder::parse(
        "<style>#parent{font-size:20px;line-height:30px;--angle:10deg;\
         --bad-size:sibling-index();--bad-line:calc(sibling-index() * 1deg);\
         --bad-number:calc(sibling-index() * 1px)}\
         #target{font-size:28px;line-height:40px;flex-grow:9;rotate:45deg}\
         #target{font-size:var(--bad-size);line-height:var(--bad-line);\
         flex-grow:var(--bad-number);rotate:calc(sibling-index(1) * var(--angle))}</style>\
         <div id='parent'><span></span><div id='target'></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const s=getComputedStyle(document.getElementById('target'));\
         return JSON.stringify([s.fontSize,s.lineHeight,s.flexGrow,s.rotate]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, r#"["20px","30px","0","none"]"#);
}

#[test]
fn descriptors_reject_nested_tree_counts_on_parse_and_cssom_assignment() {
    let document = TreeBuilder::parse(
        "<style id='sheet'>@page{margin:100px;margin:calc(0px * sibling-index())}\
         @font-face{font-family:demo;font-weight:300;font-weight:calc(max(0 * sibling-count(),400));\
         font-feature-settings:\"vert\" 2;font-feature-settings:\"vert\" calc(max(sibling-index(),1))}</style>"
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const rules=document.getElementById('sheet').sheet.cssRules;\
         const read=()=>[rules[0].style.margin,rules[1].style.fontWeight,rules[1].style.fontFeatureSettings];\
         const before=read();rules[0].style.margin='calc(1px * sibling-count())';\
         rules[1].style.setProperty('font-weight','calc(max(sibling-index(),500))');\
         rules[1].style.fontFeatureSettings='\"vert\" sibling-count()';\
         return JSON.stringify([before,read()]);})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["100px","300","\"vert\" 2"],["100px","300","\"vert\" 2"]]"#
    );
}

#[test]
fn descriptor_rule_interfaces_preserve_valid_values_before_tree_counting_declarations() {
    let document = TreeBuilder::parse(
        "<style id='sheet'>\
         @font-palette-values --demo{font-family:demo;base-palette:1;base-palette:calc(max(sibling-index(),2));\
         override-colors:1 green;override-colors:sibling-count() red}\
         @counter-style demo{system:fixed 1;system:fixed calc(max(sibling-index(),2));\
         prefix:--pass;prefix:linear-gradient(red calc(20px * sibling-count()),pink);symbols:--pass}\
         @font-feature-values demo{@swash{pretty:1}@swash{pretty:calc(max(sibling-count(),2))}}</style>"
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const [p,c,f]=document.getElementById('sheet').sheet.cssRules;\
         return JSON.stringify([[p.constructor.name,p.name,p.fontFamily,p.basePalette,p.overrideColors],\
         [c.constructor.name,c.name,c.system,c.prefix,c.symbols],\
         [f.constructor.name,f.fontFamily,[...f.swash]],[p.type,c.type,f.type]]);})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["CSSFontPaletteValuesRule","--demo","demo","1","1 green"],["CSSCounterStyleRule","demo","fixed 1","--pass","--pass"],["CSSFontFeatureValuesRule","demo",[["pretty",[1]]]],[0,11,14]]"#
    );
}

#[test]
fn font_feature_maps_compute_fixed_integer_math_and_copy_mutated_values() {
    let document = TreeBuilder::parse(
        "<style id='sheet'>@font-feature-values demo{@swash{a:calc(1 + 1);b:0;c:calc(max(1,3))}}</style>"
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const m=document.getElementById('sheet').sheet.cssRules[0].swash;\
         const initial=['a','b','c'].map(k=>m.get(k));\
         m.set('x',null);const zero=m.get('x');m.set('x',-1);const negative=m.get('x');\
         m.set('x',[]);const empty=m.get('x');m.set('x',3);const copy=m.get('x');copy[0]=99;\
         return JSON.stringify([initial,zero,negative,empty,m.get('x'),m instanceof Map]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, r#"[[[2],[0],[3]],[0],[4294967295],[],[3],false]"#);
}

#[test]
fn palette_weights_compute_after_dom_changes_and_equal_palettes_simplify() {
    let document = TreeBuilder::parse(
        "<style>#target{font-palette:palette-mix(in lch,--a calc(5% * sibling-index()),--b 80%)}</style>\
         <div><span id='first'></span><span></span><span></span><div id='target'></div></div>"
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const e=document.getElementById('target');const before=getComputedStyle(e).fontPalette;\
         document.getElementById('first').remove();const after=getComputedStyle(e).fontPalette;\
         e.style.fontPalette='palette-mix(in lch,--a 20%,--a 80%)';\
         return JSON.stringify([before,after,getComputedStyle(e).fontPalette]);})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        actual,
        r#"["palette-mix(in lch, --a 20%, --b 80%)","palette-mix(in lch, --a 15%, --b 80%)","--a"]"#
    );
}

#[test]
fn palette_mix_preserves_alpha_weights_and_supports_multiple_palettes() {
    let document = TreeBuilder::parse("<div id='target'></div>").document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');\
         const values=['palette-mix(in lch,--a 20%,--a 20%)',\
         'palette-mix(in oklab,--a 20%,--b 30%,--c 50%)',\
         'palette-mix(in lch,20% --a,--b 80%)'];\
         return JSON.stringify([values.map(v=>{e.style.fontPalette=v;\
         return [CSS.supports('font-palette',v),getComputedStyle(e).fontPalette]}),\
         CSS.supports('font-palette','palette-mix(in lch,--a 5px,--b)'),\
         CSS.supports('font-palette','palette-mix(in lch,--a sibling-index(1),--b)')]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[[[true,"palette-mix(in lch, --a 20%, --a 20%)"],[true,"palette-mix(--a 20%, --b 30%, --c 50%)"],[true,"palette-mix(in lch, --a 20%, --b 80%)"]],false,false]"#
    );
}

#[test]
fn transform_math_resolves_dom_counts_percentages_and_font_units_to_matrices() {
    let document = TreeBuilder::parse(
        "<style>html{font-size:20px}#target{width:100px;height:50px;font-size:10px;\
         transform:translateX(calc(sibling-index() * 10px)) rotate(calc(cos(sibling-index() * 0deg) * 0deg))}</style>\
         <div><span id='first'></span><div id='target'></div></div>"
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');\
         const read=()=>{const value=getComputedStyle(e).transform;\
         return value.startsWith('matrix(')?value.slice(7,-1).split(',').map(Number):value};\
         const before=read();document.getElementById('first').remove();const after=read();\
         e.style.transform='translateX(calc(20% + 10px))';const percentage=read();\
         e.style.transform='translateX(calc(1em + 1rem))';const font=read();\
         e.style.transform='scale(1e-9)';\
         return JSON.stringify([before,after,percentage,font,read(),\
         CSS.supports('transform','translateX(calc(sibling-index() * 1px))'),\
         CSS.supports('transform','translateX(sibling-index())')]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[[1,0,0,1,20,0],[1,0,0,1,10,0],[1,0,0,1,30,0],[1,0,0,1,30,0],[1e-9,0,0,1e-9,0,0],true,false]"#
    );
}

#[test]
fn pseudo_transform_matrices_use_the_originating_element_and_the_pseudos_own_box() {
    let document = TreeBuilder::parse(
        "<style>#target{width:100px;height:50px;transform:translateX(calc(sibling-index() * 10px))}\
         #target::before{content:'b';display:block;width:50px;height:20px;transform:translateX(calc(sibling-index() * 10px))}\
         #target::after{content:'a';display:block;width:50px;height:20px;transform:translateX(calc(50% + sibling-count() * 10px))}</style>\
         <div><span id='first'></span><div id='target'></div></div>"
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const e=document.getElementById('target');\
         const read=()=>['','::before','::after'].map(p=>getComputedStyle(e,p).transform);\
         const before=read();document.getElementById('first').remove();const after=read();\
         e.style.display='none';return JSON.stringify([before,after,read()]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["matrix(1, 0, 0, 1, 20, 0)","matrix(1, 0, 0, 1, 20, 0)","matrix(1, 0, 0, 1, 45, 0)"],["matrix(1, 0, 0, 1, 10, 0)","matrix(1, 0, 0, 1, 10, 0)","matrix(1, 0, 0, 1, 35, 0)"],["none","none","none"]]"#
    );
}

#[test]
fn registered_tree_counts_still_require_the_consuming_font_propertys_type() {
    let document = TreeBuilder::parse(
        "<style>@property --index{syntax:'<integer>';inherits:true;initial-value:1}\
         @property --angle{syntax:'<angle>';inherits:true;initial-value:0deg}\
         #parent{--index:sibling-index();--angle:calc(sibling-count() * 1deg);font-size:20px;line-height:30px}\
         #target{font-size:28px;line-height:40px}\
         #target{font-size:var(--index);line-height:var(--angle)}</style>\
         <div><span></span><div id='parent'><div id='target'></div></div></div>",
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval(
            "(() => {const s=getComputedStyle(document.getElementById('target'));\
         return JSON.stringify([s.fontSize,s.lineHeight,s.getPropertyValue('--index')]);})()",
        )
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, r#"["20px","30px","2"]"#);
}

#[test]
fn tree_counting_line_heights_inherit_percentages_as_lengths_and_numbers_as_factors() {
    let document = TreeBuilder::parse(
        "<style>#percentage,#number{font-size:20px}.child{font-size:10px}\
         #percentage{line-height:calc(sibling-index()*100%)}\
         #number{line-height:sibling-index()}</style>\
         <div><span></span><div id='percentage'><div class='child'></div></div></div>\
         <div><span></span><div id='number'><div class='child'></div></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "JSON.stringify(['percentage','number'].map(id=>{const e=document.getElementById(id);\
         return [getComputedStyle(e).lineHeight,getComputedStyle(e.firstElementChild).lineHeight]}))"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(actual, r#"[["40px","40px"],["40px","20px"]]"#);
}

#[test]
fn registered_zero_count_is_a_valid_zero_font_length() {
    let document = TreeBuilder::parse(
        "<style>@property --zero{syntax:'<integer>';inherits:true;initial-value:0}\
         #target{--zero:calc(sibling-index() - sibling-index());font-size:var(--zero)}</style>\
         <div id='target'><span id='child'></span></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "JSON.stringify(['target','child'].map(id=>{const s=getComputedStyle(document.getElementById(id));return [s.fontSize,s.getPropertyValue('--zero')]}))"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(actual, r#"[["0px","0"],["0px","0"]]"#);
}

#[test]
fn tree_counting_math_cancels_units_and_preserves_compound_products() {
    let document = TreeBuilder::parse(
        "<style>#target{flex-grow:calc(sibling-index() * 1px / 1px);\
         flex-shrink:calc(sibling-index() * 1s / 1000ms);\
         order:calc(sibling-index() * 1deg / 1deg);\
         width:calc(sibling-index() * 1px * 1px / 1px)}</style>\
         <div><span id='first'></span><div id='target'></div></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const e=document.getElementById('target');const read=()=>{const s=getComputedStyle(e);\
         return [s.flexGrow,s.flexShrink,s.order,s.width]};const before=read();\
         document.getElementById('first').remove();return JSON.stringify([before,read()])})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(actual, r#"[["2","2","2","2px"],["1","1","1","1px"]]"#);
}

#[test]
fn negative_tree_counting_font_math_clamps_at_computed_value_time() {
    let document = TreeBuilder::parse(
        "<style>#target{font-size:calc(sibling-count() * -1px)}</style><div id='target'></div>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime
        .eval("getComputedStyle(document.getElementById('target')).fontSize")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    assert_eq!(actual, "0px");
}

#[test]
fn standalone_numeric_functions_compute_tree_counts_and_registered_numbers() {
    let document = TreeBuilder::parse(
        "<style>@property --number{syntax:'<number>';inherits:true;initial-value:0}\
         #target{--number:hypot(sibling-index(),0);flex-grow:hypot(sibling-index(),0);\
         flex-shrink:var(--number);opacity:abs(calc(sibling-index() * -.1));\
         rotate:atan2(sibling-index(),0);font-size:abs(calc(sibling-index() * -10px));\
         width:mod(calc(sibling-index() * 10px),15px);height:rem(calc(sibling-index() * 10px),15px)}</style>\
         <div><span id='first'></span><div id='target'></div></div>",
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const e=document.getElementById('target');const read=()=>{const s=getComputedStyle(e);\
         return [s.flexGrow,s.flexShrink,s.opacity,s.rotate,s.fontSize,s.width,s.height]};const before=read();\
         document.getElementById('first').remove();return JSON.stringify([before,read()])})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["2","2","0.2","90deg","20px","5px","5px"],["1","1","0.1","90deg","10px","10px","10px"]]"#
    );
}

#[test]
fn integer_registered_counts_round_math_and_animated_intermediate_values() {
    let document = TreeBuilder::parse(
        "<style>@property --rounded{syntax:'<integer>';inherits:true;initial-value:0}\
         @property --animated{syntax:'<integer>';inherits:false;initial-value:0}\
         @keyframes integer-count{from{--animated:sibling-index()}to{--animated:calc(sibling-index() * 2)}}\
         #target{--rounded:calc(sibling-index() * .25);order:var(--rounded);opacity:sibling-count();\
         animation:integer-count 1s linear -.5s paused;width:calc(var(--animated) * 10px)}</style>\
         <div><span id='first'></span><div id='target'></div></div>",
    ).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    let actual = runtime.eval(
        "(() => {const e=document.getElementById('target');const read=()=>{const s=getComputedStyle(e);\
         return [s.order,s.opacity,s.getPropertyValue('--rounded'),s.getPropertyValue('--animated'),s.width]};\
         const before=read();document.getElementById('first').remove();return JSON.stringify([before,read()])})()"
    ).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(
        actual,
        r#"[["1","1","1","3","30px"],["0","1","0","2","20px"]]"#
    );
}
