use crate::{JsNativeErrorKind, TestAction, run_test_actions};
use indoc::indoc;

#[test]
fn let_is_block_scoped() {
    run_test_actions([TestAction::assert_native_error(
        indoc! {r#"
            {
              let bar = "bar";
            }
            bar;
        "#},
        JsNativeErrorKind::Reference,
        "bar is not defined",
    )]);
}

#[test]
fn const_is_block_scoped() {
    run_test_actions([TestAction::assert_native_error(
        indoc! {r#"
            {
            const bar = "bar";
            }
            bar;
        "#},
        JsNativeErrorKind::Reference,
        "bar is not defined",
    )]);
}

#[test]
fn var_not_block_scoped() {
    run_test_actions([TestAction::assert(indoc! {r#"
            {
              var bar = "bar";
            }
            bar == "bar";
        "#})]);
}

#[test]
fn functions_use_declaration_scope() {
    run_test_actions([TestAction::assert_native_error(
        indoc! {r#"
            function foo() {
                bar;
            }
            {
                let bar = "bar";
                foo();
            }
        "#},
        JsNativeErrorKind::Reference,
        "bar is not defined",
    )]);
}

#[test]
fn set_outer_var_in_block_scope() {
    run_test_actions([TestAction::assert(indoc! {r#"
            var bar;
            {
                bar = "foo";
            }
            bar == "foo";
        "#})]);
}

#[test]
fn set_outer_let_in_block_scope() {
    run_test_actions([TestAction::assert(indoc! {r#"
            let bar;
            {
                bar = "foo";
            }
            bar == "foo";
        "#})]);
}

#[test]
fn direct_eval_in_nested_class_preserves_outer_binding() {
    run_test_actions([TestAction::assert(indoc! {r#"
        class O {
            run() {
                const outer = 123;
                return new class I {
                    run() { return eval('outer'); }
                }().run();
            }
        }
        new O().run() === 123;
    "#})]);
}

#[test]
fn direct_eval_in_nested_class_preserves_outer_private_receiver() {
    run_test_actions([TestAction::assert(indoc! {r#"
        class O {
            #x = 7;
            run() {
                const outer = this;
                return new class I {
                    #y = 31;
                    run() { return eval('outer.#x'); }
                }().run();
            }
        }
        new O().run() === 7;
    "#})]);
}

#[test]
fn direct_eval_in_nested_members_preserves_outer_binding() {
    run_test_actions([TestAction::assert(indoc! {r#"
        function objectMethod() {
            const outer = 123;
            return { run() { return eval('outer'); } }.run();
        }
        function objectGetter() {
            const outer = 123;
            return { get value() { return eval('outer'); } }.value;
        }
        function classDeclaration() {
            const outer = 123;
            class I { run() { return eval('outer'); } }
            return new I().run();
        }
        function classField() {
            const outer = 123;
            return new class { value = eval('outer'); }().value;
        }
        function privateField() {
            const outer = 123;
            return new class {
                #value = eval('outer');
                get value() { return this.#value; }
            }().value;
        }
        function classStaticBlock() {
            const outer = 123;
            class I { static value; static { this.value = eval('outer'); } }
            return I.value;
        }
        [objectMethod, objectGetter, classDeclaration, classField,
         privateField, classStaticBlock].every(fn => fn() === 123);
    "#})]);
}

#[test]
fn direct_eval_in_nested_method_parameters_preserves_outer_binding() {
    run_test_actions([TestAction::assert(indoc! {r#"
        function classDefault() {
            const outer = 123;
            return new class {
                run(value = eval('outer')) { return value; }
            }().run();
        }
        function objectDefault() {
            const outer = 123;
            return { run(value = eval('outer')) { return value; } }.run();
        }
        classDefault() === 123 && objectDefault() === 123;
    "#})]);
}

#[test]
fn direct_eval_in_computed_method_names_preserves_outer_binding() {
    run_test_actions([TestAction::assert(indoc! {r#"
        function classComputed() {
            const outer = 'method';
            return new class {
                [eval('outer')]() { return 7; }
            }().method();
        }
        function objectComputed() {
            const outer = 'method';
            return { [eval('outer')]() { return 7; } }.method();
        }
        classComputed() === 7 && objectComputed() === 7;
    "#})]);
}

#[test]
fn direct_eval_resolves_shadowed_and_outer_private_names() {
    run_test_actions([TestAction::assert(indoc! {r#"
        class Outer {
            #shared = 11;
            #outerOnly = 7;
            run() {
                const outer = this;
                return new class Inner {
                    #shared = 31;
                    run() {
                        let wrongBrand = false;
                        try { eval('outer.#shared'); }
                        catch (error) { wrongBrand = error instanceof TypeError; }
                        return eval('this.#shared') === 31 &&
                            eval('outer.#outerOnly') === 7 && wrongBrand;
                    }
                }().run();
            }
        }
        new Outer().run();
    "#})]);
}

#[test]
fn eval_rejects_unavailable_private_names_before_execution() {
    run_test_actions([TestAction::assert(indoc! {r#"
        globalThis.privateEvalEffect = 0;
        class Subject {
            #known = 42;
            run() {
                let directRejected = false;
                let indirectRejected = false;
                try { eval('globalThis.privateEvalEffect = 1; this.#missing'); }
                catch (error) { directRejected = error instanceof SyntaxError; }
                try { (0, eval)('globalThis.privateEvalEffect = 2; this.#known'); }
                catch (error) { indirectRejected = error instanceof SyntaxError; }
                return directRejected && indirectRejected &&
                    globalThis.privateEvalEffect === 0 && eval('this.#known') === 42;
            }
        }
        new Subject().run();
    "#})]);
}
