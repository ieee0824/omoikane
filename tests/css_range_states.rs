//! Range selectors must agree with sanitized values and native form state.
use omoikane::{
    css::{matches_selector, parse_selector_list},
    html::TreeBuilder,
    js::JsRuntime,
};

fn evaluate(html: &str, script: &str) -> String {
    let mut runtime = JsRuntime::with_document(TreeBuilder::parse(html).document()).unwrap();
    let program = format!(
        "(() => {{ try {{ {script} }} catch (error) {{ return error.name + ': ' + error.message; }} }})()"
    );
    runtime
        .eval(&program)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn range_grammar_matches_supports_cssom_and_dom_queries() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      for(const name of ['in-range','out-of-range']){
        if(!CSS.supports(`selector(:${name})`))return 'unsupported-'+name;
        for(const selector of [`:${name}()`, `:${name}(x)`]){
          if(CSS.supports(`selector(${selector})`))return 'functional-support';
          const sheet=new CSSStyleSheet();sheet.replaceSync(`${selector}{color:red}`);
          if(sheet.cssRules.length)return 'functional-cssom';
          try{document.querySelector(selector);return 'functional-query';}catch(e){if(e.name!=='SyntaxError')return 'query-error';}
        }
      }
      return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn number_range_states_follow_limits_empty_values_and_validation_barring() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const n=document.createElement('input');n.type='number';n.min='0';n.max='10';n.value='5';document.body.append(n);
      const state=()=>[n.matches(':in-range'),n.matches(':out-of-range')].join(',');
      if(state()!=='true,false')return 'bounded';
      n.value='11';if(state()!=='false,true')return 'overflow';
      n.value='';if(state()!=='true,false')return 'empty';
      n.readOnly=true;if(state()!=='false,false')return 'readonly';n.readOnly=false;
      n.disabled=true;if(state()!=='false,false')return 'disabled';n.disabled=false;
      const list=document.createElement('datalist');list.append(n);document.body.append(list);
      if(state()!=='false,false')return 'datalist';document.body.append(n);
      const field=document.createElement('fieldset');field.disabled=true;field.append(n);document.body.append(field);
      if(state()!=='false,false')return 'fieldset';
      const legend=document.createElement('legend');legend.append(n);field.prepend(legend);
      if(state()!=='true,false')return 'legend';document.body.append(n);
      n.removeAttribute('min');n.removeAttribute('max');if(state()!=='false,false')return 'unlimited';
      n.min='10';n.max='0';n.value='5';if(state()!=='false,true')return 'reversed-number';
      n.value='';return state()==='true,false'?'ok':'reversed-empty';
    "#
        ),
        "ok"
    );
}

#[test]
fn calendar_and_periodic_time_states_use_the_current_typed_value() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const rows=[['date','2020-01-01','2020-12-31','2020-06-01','2019-01-01','2021-01-01'],['month','2020-01','2020-12','2020-06','2019-12','2021-01'],['week','2020-W01','2020-W52','2020-W20','2019-W52','2021-W01'],['time','01:00','10:00','05:00','00:00','11:00'],['datetime-local','2020-01-01T00:00','2020-12-31T23:59','2020-06-01T00:00','2019-12-31T23:59','2021-01-01T00:00']];
      for(const [type,min,max,inside,under,over] of rows){
        const n=document.createElement('input');n.type=type;n.min=min;n.max=max;
        n.value=inside;if(!n.matches(':in-range')||n.matches(':out-of-range'))return type+'-inside';
        for(const value of [under,over]){n.value=value;if(!n.matches(':out-of-range')||n.matches(':in-range'))return type+'-outside';}
        n.value='bad';if(n.value!==''||!n.matches(':in-range'))return type+'-sanitize';
      }
      const t=document.createElement('input');t.type='time';t.min='21:00';t.max='03:00';
      for(const value of ['21:00','22:00','02:00','03:00']){t.value=value;if(!t.matches(':in-range'))return 'wrap-inside';}
      t.value='12:00';if(!t.matches(':out-of-range')||!t.validity.rangeUnderflow||!t.validity.rangeOverflow)return 'wrap-gap';
      t.value='bad';return t.value===''&&t.matches(':in-range')?'ok':'wrap-empty';
    "#
        ),
        "ok"
    );
}

#[test]
fn range_value_sanitization_agrees_with_selectors_and_validity() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const n=document.createElement('input');n.type='range';
      if(n.value!=='50')return 'default';
      n.min='0';n.max='10';n.value='99';
      if(n.value!=='10'||n.valueAsNumber!==10||n.validity.rangeOverflow||!n.matches(':in-range'))return 'clamp:'+JSON.stringify([n.value,n.valueAsNumber,n.validity.rangeOverflow,n.matches(':in-range')]);
      n.value='5.5';if(n.value!=='6')return 'round';
      n.step='any';n.value='5.5';if(n.value!=='5.5')return 'any';
      n.step='2';if(n.value!=='6')return 'step-change';
      n.step='any';n.min='7';n.max='2';n.value='';
      if(n.value!=='7'||!n.matches(':out-of-range')||!n.validity.rangeOverflow)return 'reversed';
      n.readOnly=true;return !n.willValidate&&!n.matches(':in-range')&&!n.matches(':out-of-range')?'ok':'readonly';
    "#
        ),
        "ok"
    );
}

#[test]
fn dirty_range_values_cloning_reset_and_relational_styles_stay_live() {
    assert_eq!(
        evaluate(
            "<style>input{color:green;background-color:transparent}input:out-of-range{color:red}input:nth-child(2 of :in-range){background-color:yellow}form{background-color:transparent}form:has(:out-of-range){background-color:blue}</style><form id=f><input id=a type=number min=0 max=10 value=5><input id=b type=number min=0 max=10 value=50></form>",
            r#"
      const f=document.getElementById('f'),a=document.getElementById('a'),b=document.getElementById('b'),bg=n=>getComputedStyle(n).backgroundColor;
      if(getComputedStyle(b).color!=='rgb(255, 0, 0)'||bg(f)!=='rgb(0, 0, 255)')return 'initial';
      a.value='20';b.value='2';if(!a.matches(':out-of-range')||!b.matches(':in-range'))return 'dirty';
      a.value='3';if(bg(b)!=='rgb(255, 255, 0)'||bg(f)!=='rgba(0, 0, 0, 0)')return 'styles';
      for(const n of [a.cloneNode(true),document.importNode(a,true)]){n.max='2';if(n.value!=='3'||!n.matches(':out-of-range'))return 'clone';}
      f.reset();return a.value==='5'&&b.value==='50'&&a.matches(':in-range')&&b.matches(':out-of-range')?'ok':'reset';
    "#
        ),
        "ok"
    );
}

#[test]
fn range_matching_uses_html_namespaces_and_internal_values() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const H='http://www.w3.org/1999/xhtml',doc=document.implementation.createDocument(H,'html',null),n=doc.createElementNS(H,'h:input');
      n.type='number';n.setAttributeNS('urn:other','min','50');n.min='0';n.max='10';n.value='20';doc.documentElement.append(n);
      if(n.getAttributeNS(null,'min')!=='0'||n.getAttributeNS('urn:other','min')!=='50')return 'min-namespace';
      if(!n.matches('input:out-of-range'))return 'prefix';
      n.disabled=true;if(n.matches(':out-of-range'))return 'disabled-prefix';n.disabled=false;
      for(const ns of [null,'http://www.w3.org/2000/svg']){const x=doc.createElementNS(ns,'input');x.setAttribute('type','number');x.setAttribute('min','0');x.setAttribute('max','10');x.setAttribute('value','20');if(x.matches(':in-range,:out-of-range'))return 'namespace';}
      for(const key of ['value','type','min','max'])Object.defineProperty(n,key,{get(){throw new Error('author getter '+key)}});
      return n.matches(':out-of-range')?'ok':'private-state';
    "#
        ),
        "ok"
    );
}

#[test]
fn parsed_numeric_range_matches_without_a_javascript_runtime() {
    let doc = TreeBuilder::parse("<input id=n type=number min=0 max=10 value=20>").document();
    let n = doc.query_selector("#n").unwrap();
    let selector = parse_selector_list(":out-of-range").unwrap();
    assert!(matches_selector(&n, &selector[0]));
}

#[test]
fn range_keeps_aligned_decimal_spelling_and_rounds_decimal_ties_up() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const n=document.createElement('input');n.type='range';n.min='0';n.max='10';n.step='any';n.value='5.00';
      if(n.value!=='5.00')return 'lexical:'+n.value;
      n.max='1';n.step='.1';
      for(const [value,expected] of [['.3','.3'],['.2','.2'],['0.30000000000000004','0.3'],['0.35','0.4']]){
        n.value=value;if(n.value!==expected)return value+':'+n.value;
      }
      return 'ok';
    "#
        ),
        "ok"
    );
}

#[test]
fn dirty_range_values_survive_default_value_and_same_attribute_writes() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const n=document.createElement('input');n.type='range';n.max='10';n.step='2';n.value='5';
      n.defaultValue='1';if(n.value!=='6'||n.valueAsNumber!==6||!n.matches(':in-range'))return 'base';
      n.step='2';if(n.value!=='6')return 'unchanged-attribute';
      n.defaultValue='0';n.value='6';n.stepUp(-1);if(n.value!=='4')return 'negative-up';
      n.stepDown(-1);return n.value==='6'?'ok':'negative-down';
    "#
        ),
        "ok"
    );
}

#[test]
fn overflow_computed_style_serializes_initial_and_explicit_axes() {
    assert_eq!(
        evaluate(
            "<body><input id=n type=range></body>",
            r#"
      const n=document.getElementById('n'),style=getComputedStyle(n);
      if(style.overflow!=='visible'||style.overflowX!=='visible'||style.overflowY!=='visible')return 'initial';
      n.style.overflow='hidden scroll';
      if(style.overflow!=='hidden scroll'||style.overflowX!=='hidden'||style.overflowY!=='scroll')return 'axes';
      n.style.overflow='hidden';if(style.getPropertyValue('overflow')!=='hidden')return 'live';
      n.remove();return style.overflow===''&&style.overflowX===''&&style.overflowY===''?'ok':'detached:'+style.overflow;
    "#
        ),
        "ok"
    );
}

#[test]
fn range_numeric_attributes_use_html_parsing_and_value_syntax_stays_strict() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const n=document.createElement('input');n.type='number';n.min=' +5';n.value='0';
      if(!n.matches(':out-of-range')||!n.validity.rangeUnderflow)return 'attribute';
      n.value='+5';if(n.value!==''||!Number.isNaN(n.valueAsNumber)||!n.matches(':in-range'))return 'value';
      n.type='text';try{n.valueAsNumber=1;return 'unsupported'}catch(e){if(e.name!=='InvalidStateError')return e.name}
      n.type='number';for(const value of [Infinity,-Infinity,1n]){try{n.valueAsNumber=value;return 'accepted'}catch(e){if(e.name!=='TypeError')return e.name}}
      n.valueAsNumber=NaN;return n.value===''?'ok':'nan';
    "#
        ),
        "ok"
    );
}

#[test]
fn validation_boolean_reflection_ignores_foreign_attributes() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const n=document.createElement('input');n.type='number';n.min='0';n.value='-1';
      for(const name of ['disabled','readonly','required'])n.setAttributeNS('urn:other',name,'foreign');
      if(n.disabled||n.readOnly||n.required||!n.willValidate||!n.matches(':out-of-range'))return 'foreign';
      n.disabled=true;if(!n.disabled||n.willValidate||n.matches(':out-of-range')||n.getAttributeNS(null,'disabled')===null)return 'disabled';
      n.disabled=false;n.readOnly=true;
      if(!n.readOnly||n.willValidate||n.matches(':out-of-range')||n.getAttributeNS(null,'readonly')===null)return 'readonly';
      n.readOnly=false;n.required=true;
      if(!n.required||!n.matches(':required')||n.getAttributeNS(null,'required')===null)return 'required';
      for(const name of ['disabled','readonly','required'])if(n.getAttributeNS('urn:other',name)!=='foreign')return 'lost-foreign';
      return n.willValidate&&n.matches(':out-of-range')?'ok':'restored';
    "#
        ),
        "ok"
    );
}

#[test]
fn range_validity_recognizes_prefixed_html_controls_and_ancestors() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const H='http://www.w3.org/1999/xhtml',doc=document.implementation.createDocument(H,'h:html',null);
      const n=doc.createElementNS(H,'h:input');n.type='number';n.min='0';n.max='10';n.value='20';doc.documentElement.append(n);
      if(!n.willValidate||!n.validity.rangeOverflow||!n.matches(':out-of-range'))return 'prefixed-input';
      n.disabled=true;if(n.willValidate||n.matches(':out-of-range'))return 'prefixed-disabled';n.disabled=false;
      const list=doc.createElementNS(H,'h:datalist');list.append(n);doc.documentElement.append(list);
      if(n.willValidate||n.matches(':out-of-range'))return 'prefixed-datalist';
      doc.documentElement.append(n);return n.willValidate&&n.validity.rangeOverflow?'ok':'restored';
    "#
        ),
        "ok"
    );
}

#[test]
fn range_validity_step_uses_null_namespace_attributes_and_exact_decimal_values() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const n=document.createElement('input');n.type='number';
      n.setAttributeNS('urn:other','min','2');n.setAttributeNS('urn:other','step','3');
      n.min='0';n.max='10';n.step='.1';n.value='.3';
      if(n.validity.stepMismatch||!n.validity.valid||!n.matches(':in-range'))return 'foreign-step';
      n.value='0.30000000000000004';if(!n.validity.stepMismatch)return 'exact-step';
      n.value='.6';return !n.validity.stepMismatch?'ok':'aligned-step';
    "#
        ),
        "ok"
    );
}

#[test]
fn range_validity_retains_default_maximum_when_minimum_reverses_it() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const n=document.createElement('input');n.type='range';n.min='200';n.value='';
      if(n.value!=='200'||!n.matches(':out-of-range')||!n.validity.rangeOverflow)return 'default-maximum';
      n.max='300';return n.matches(':in-range')&&!n.validity.rangeOverflow?'ok':'explicit-maximum';
    "#
        ),
        "ok"
    );
}

#[test]
fn range_validity_requiredness_ignores_foreign_required_attributes() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const n=document.createElement('input');n.type='number';n.min='0';n.setAttributeNS('urn:other','required','foreign');n.value='';
      if(n.required||n.validity.valueMissing||!n.validity.valid||!n.matches(':in-range')||!n.matches(':optional'))return 'foreign-required';
      n.required=true;
      if(!n.validity.valueMissing||n.validity.valid||!n.matches(':in-range')||!n.matches(':required'))return 'required';
      n.required=false;return !n.validity.valueMissing&&n.getAttributeNS('urn:other','required')==='foreign'?'ok':'restored';
    "#
        ),
        "ok"
    );
}

#[test]
fn fractional_time_values_use_exact_integer_milliseconds() {
    assert_eq!(
        evaluate(
            "<body></body>",
            r#"
      const t=document.createElement('input');t.type='time';t.step='.001';
      for(const millis of [1,9,99,999,1001,1002,1007,1999,43210999,86399999]){
        t.valueAsNumber=millis;
        if(t.valueAsNumber!==millis)return 'roundtrip:'+millis+':'+t.valueAsNumber;
        if(t.validity.stepMismatch)return 'step:'+millis;
      }
      t.min='00:00:01.001';t.max='00:00:01.003';
      for(const value of ['00:00:01.001','00:00:01.002','00:00:01.003']){
        t.value=value;if(!t.matches(':in-range')||t.validity.stepMismatch)return 'boundary:'+value;
      }
      t.value='00:00:01.004';if(!t.matches(':out-of-range')||!t.validity.rangeOverflow)return 'overflow';
      const d=document.createElement('input');d.type='datetime-local';d.step='.001';d.value='1970-01-01T00:00:01.001';
      return d.valueAsNumber===1001&&!d.validity.stepMismatch?'ok':'datetime';
    "#
        ),
        "ok"
    );
}
