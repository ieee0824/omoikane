//! Parsed radio checkedness must agree before scripts or selector evaluation.
use omoikane::{
    css::{matches_selector, parse_selector_list},
    dom::Node,
    html::TreeBuilder,
    js::JsRuntime,
};

#[test]
fn parsed_radios_choose_last_checked_without_javascript() {
    let doc = TreeBuilder::parse("<form><input id=a type=radio name=g checked><input id=b type=radio name=g checked></form><input id=c type=radio name=g checked>").document();
    let a = doc.query_selector("#a").unwrap();
    let b = doc.query_selector("#b").unwrap();
    let c = doc.query_selector("#c").unwrap();
    assert!(
        !a.checked(),
        "earlier checked radio must be cleared by parsing"
    );
    assert!(b.checked());
    assert!(c.checked(), "different form owner keeps its checkedness");
    let default = parse_selector_list(":default").unwrap();
    assert!(matches_selector(&a, &default[0]));
    assert!(matches_selector(&b, &default[0]));
    b.set_checked(false);
    let indeterminate = parse_selector_list(":indeterminate").unwrap();
    assert!(matches_selector(&a, &indeterminate[0]));
    assert!(matches_selector(&b, &indeterminate[0]));
    assert!(!matches_selector(&c, &indeterminate[0]));
}

#[test]
fn parsed_radio_owners_and_reset_preserve_content_defaults() {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse("<input id=x type=radio name=h form=f checked><form id=f><input id=a type=radio name=g checked><input id=b type=radio name=g checked><input id=y type=radio name=h checked></form><input id=q type=radio name=g checked>").document()).unwrap();
    let result = runtime.eval(r#"(() => {
      const f=document.getElementById('f'),a=document.getElementById('a'),b=document.getElementById('b'),x=document.getElementById('x'),y=document.getElementById('y'),q=document.getElementById('q');
      if(a.checked||!b.checked||x.checked||!y.checked||!q.checked)return 'initial';
      if(![a,b,x,y].every(n=>n.defaultChecked&&n.matches(':default')))return 'content-defaults';
      b.checked=false;y.checked=false;
      if(![a,b,x,y].every(n=>n.matches(':indeterminate'))||q.matches(':indeterminate'))return 'cleared';
      f.reset();
      return !a.checked&&b.checked&&!x.checked&&y.checked&&q.checked&&[a,b,x,y].every(n=>n.defaultChecked)?'ok':'reset';
    })()"#).unwrap().as_string().unwrap().to_std_string_escaped();
    assert_eq!(result, "ok");
}

#[test]
fn foster_parented_radios_follow_insertion_order_not_final_tree_order() {
    let doc = TreeBuilder::parse("<form><table id=t><input id=a type=radio name=g checked><tbody><tr><td><input id=b type=radio name=g checked></td></tr></tbody><input id=c type=radio name=g checked></table></form>").document();
    let a = doc.query_selector("#a").unwrap();
    let b = doc.query_selector("#b").unwrap();
    let c = doc.query_selector("#c").unwrap();
    let table = doc.query_selector("#t").unwrap();
    let siblings = c.parent_node().unwrap().child_nodes();
    let c_position = siblings.iter().position(|n| n == &c).unwrap();
    let table_position = siblings.iter().position(|n| n == &table).unwrap();
    assert!(c_position < table_position);
    assert!(!a.checked());
    assert!(
        !b.checked(),
        "the later DOM descendant was inserted before c"
    );
    assert!(c.checked(), "the last inserted radio wins");
}

#[test]
fn written_radios_can_replace_a_selected_later_dom_member() {
    let doc = TreeBuilder::parse(r#"<style>input{color:green}input:checked{color:red}</style><form id=f><script>const b=document.createElement('input');b.id='b';b.type='radio';b.name='g';b.defaultChecked=true;b.checked=true;document.getElementById('f').append(b);window.radioBefore=getComputedStyle(b).color;document.write('<input id=a type=radio name=g checked>');window.radioAfter=getComputedStyle(b).color;</script></form>"#).document();
    let mut runtime = JsRuntime::with_document(doc).unwrap();
    let errors = runtime.execute_document_scripts(None);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(runtime.eval("document.getElementById('a').checked&&!document.getElementById('b').checked&&document.getElementById('b').defaultChecked&&window.radioBefore==='rgb(255, 0, 0)'&&window.radioAfter==='rgb(0, 128, 0)'").unwrap().as_boolean(),Some(true));
}

#[test]
fn merged_root_id_reassociates_external_checked_radios() {
    let doc = TreeBuilder::parse("<input id=q type=radio name=g checked><form id=f><input id=x type=radio name=g form=f checked></form><html id=f>").document();
    let q = doc.query_selector("#q").unwrap();
    let x = doc.query_selector("#x").unwrap();
    assert!(!q.checked(), "root ID blocks the form and merges x with q");
    assert!(x.checked());
    x.set_checked(false);
    let indeterminate = parse_selector_list(":indeterminate").unwrap();
    assert!(matches_selector(&q, &indeterminate[0]));
    assert!(matches_selector(&x, &indeterminate[0]));
}

#[test]
fn xhtml_parser_initializes_prefixed_html_radio_groups() {
    let doc = omoikane::xml::parse(br#"<h:html xmlns:h="http://www.w3.org/1999/xhtml"><h:form><h:input id="a" type="radio" name="g" checked=""/><h:input id="b" type="radio" name="g" checked=""/></h:form></h:html>"#).unwrap();
    let a = doc.query_selector("#a").unwrap();
    let b = doc.query_selector("#b").unwrap();
    assert!(
        !a.checked(),
        "XML-created HTML inputs also normalize peer checkedness"
    );
    assert!(b.checked());
    b.set_checked(false);
    let indeterminate = parse_selector_list(":indeterminate").unwrap();
    assert!(matches_selector(&a, &indeterminate[0]));
    assert!(matches_selector(&b, &indeterminate[0]));
}
