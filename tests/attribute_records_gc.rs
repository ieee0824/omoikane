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
