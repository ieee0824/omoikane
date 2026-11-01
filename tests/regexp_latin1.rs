//! RegExp must preserve ECMAScript semantics for compact Latin1 strings.
use omoikane::js::JsRuntime;

fn evaluate(source: &str) -> String {
    JsRuntime::new()
        .unwrap()
        .eval(source)
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn latin1_sticky_lookbehind_preserves_capture_offsets_and_backreferences() {
    assert_eq!(
        evaluate(
            r#"(() => {
            const expression = /(?<=é)(?<first>É)(é)\k<first>/diy;
            expression.lastIndex = 1;
            const match = expression.exec('éÉéé');
            return JSON.stringify([match[0], match.groups.first, match.index,
                expression.lastIndex, match.indices[0], match.indices.groups.first,
                match.input]);
        })()"#
        ),
        r#"["Ééé","É",1,4,[1,4],[1,2],"éÉéé"]"#
    );
}

#[test]
fn latin1_matching_retains_unicode_and_legacy_case_folding() {
    assert_eq!(
        evaluate(
            r#"JSON.stringify(['i', 'iu', 'iv'].map(flags => [
            new RegExp('\u017f', flags).test('s'),
            new RegExp('\u212a', flags).test('k'),
            new RegExp('\u00c9', flags).test('\u00e9')
        ]))"#
        ),
        "[[false,false,true],[true,true,true],[true,true,true]]"
    );
}

#[test]
fn global_match_indices_cover_every_latin1_code_unit() {
    assert_eq!(
        evaluate(
            r#"(() => {
            const text = String.fromCharCode(...Array.from({length:256}, (_, i) => i));
            const matches = [...text.matchAll(/(?<unit>[\s\S])/dgu)];
            return matches.length === 256 && matches.every((match, index) =>
                match.index === index && match[0].charCodeAt(0) === index &&
                match.groups.unit === match[0] && match.input === text &&
                match.indices.groups.unit[0] === index &&
                match.indices.groups.unit[1] === index + 1) ? 'ok' : 'bad indices';
        })()"#
        ),
        "ok"
    );
}

#[test]
fn anchors_search_and_surrogate_positions_keep_their_existing_contracts() {
    assert_eq!(
        evaluate(
            r#"(() => {
            const anchored = /^x/my;
            anchored.lastIndex = 1;
            const atLine = anchored.exec('\nxé');
            const atStart = /^é/y;
            atStart.lastIndex = 1;
            const failed = atStart.exec('aé');
            const global = /x/g;
            const ahead = global.exec('éx');
            const sticky = /x/y;
            const noSearch = sticky.exec('éx');
            const unicode = /./uy;
            unicode.lastIndex = 1;
            const whole = unicode.exec('é😀z');
            const units = /./y;
            units.lastIndex = 1;
            const unit = units.exec('é😀z');
            return JSON.stringify([atLine.index, failed, atStart.lastIndex,
                ahead.index, global.lastIndex, noSearch, sticky.lastIndex,
                whole[0].length, unicode.lastIndex, unit[0].length, units.lastIndex]);
        })()"#
        ),
        "[1,null,0,1,2,null,0,2,3,1,2]"
    );
}

#[test]
fn legacy_and_unicode_classes_respect_the_ascii_boundary() {
    assert_eq!(
        evaluate(
            r#"JSON.stringify(['i', 'iu', 'iv'].map(flags => [
            new RegExp('[\u017f]', flags).test('s'),
            new RegExp('[s]', flags).test('\u017f'),
            new RegExp('[^s]', flags).test('\u017f'),
            new RegExp('[\u0131]', flags).test('i'),
            new RegExp('[\u039c]', flags).test('\u00b5')
        ]))"#
        ),
        "[[false,false,true,false,true],[true,true,false,false,true],[true,true,false,false,true]]"
    );
}

#[test]
fn backreferences_use_the_same_case_canonicalization_as_literals() {
    assert_eq!(
        evaluate(
            r#"JSON.stringify([
            /(s)\1/iy.test('s\u017f'),
            /(s)\1/iuy.test('s\u017f'),
            /(k)\1/ivy.test('k\u212a')
        ])"#
        ),
        "[false,true,true]"
    );
}

#[test]
fn unicode_sets_preserves_its_flag_and_rejects_legacy_identity_escapes() {
    assert_eq!(
        evaluate(
            r#"(() => {
            const expression = new RegExp('.', 'v');
            const rejected = ['\\a', '\\1', '\\01'].every(pattern => {
                try { new RegExp(pattern, 'v'); return false; }
                catch (error) { return error instanceof SyntaxError; }
            });
            return JSON.stringify([expression.unicode, expression.unicodeSets,
                expression.flags, rejected]);
        })()"#
        ),
        "[false,true,\"v\",true]"
    );
}
