//! GC retention regression tests for native attribute record conversion.

use omoikane::js::JsRuntime;

#[test]
fn native_attribute_rows_remain_live_while_building_the_outer_array() {
    let actual = JsRuntime::new()
        .expect("create attribute record runtime")
        .eval(
            r#"(() => {
                const element = document.createElement('input');
                for (let index = 0; index < 24; index++) {
                    element.setAttribute('data-' + index, 'value-' + index);
                }
                const first = element.attributes.item(0);
                for (let round = 0; round < 500; round++) {
                    const attributes = element.attributes;
                    if (attributes.length !== 24 || attributes.item(0) !== first) {
                        return 'unstable';
                    }
                    for (let index = 0; index < attributes.length; index++) {
                        if (attributes.item(index).value !== 'value-' + index) {
                            return 'wrong-value';
                        }
                    }
                }
                return first.name + '|' + first.value + '|' + element.attributes.length;
            })()"#,
        )
        .expect("evaluate attribute record probe")
        .as_string()
        .expect("attribute record probe must return a string")
        .to_std_string_escaped();

    assert_eq!(actual, "data-0|value-0|24");
}

#[test]
fn indexed_attributes_and_namespace_values_remain_live_after_mutations() {
    let actual = JsRuntime::new()
        .expect("create attribute runtime")
        .eval(
            r#"(() => {
                const element = document.createElement('div');
                element.setAttribute('data-a', 'first');
                const first = element.attributes.item(0);
                element.setAttribute('data-b', 'second');
                const second = element.attributes.item(1);
                element.setAttribute('data-a', 'updated');
                const live = [element.attributes.length, element.attributes.item(0) === first,
                              first.value, second.value].join('|');
                element.removeAttribute('data-a');
                const removed = [element.attributes.length, element.attributes.item(0) === second,
                                 element.attributes.item(1) === null, first.value].join('|');
                element.setAttributeNS('urn:test', 'p:kind', 'namespaced');
                element.setAttribute('xlink:href', 'legacy');
                const ns = [element.getAttributeNS('urn:test', 'kind'),
                            element.getAttributeNS('http://www.w3.org/1999/xlink', 'href'),
                            element.getAttributeNS(null, 'href') === null].join('|');
                return live + ';' + removed + ';' + ns;
            })()"#,
        )
        .expect("evaluate live attribute probe")
        .as_string()
        .expect("attribute probe must return a string")
        .to_std_string_escaped();

    assert_eq!(
        actual,
        "2|true|updated|second;1|true|true|updated;namespaced|legacy|true"
    );
}
