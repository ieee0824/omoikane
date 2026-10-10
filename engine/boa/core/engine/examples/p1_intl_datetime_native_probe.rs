//! Native JavaScript DateTimeFormat contracts before full Test262 validation.
use boa_engine::{Context, Source};

fn main() {
    let mut context = Context::default();
    let result = context.eval(Source::from_bytes(r#"
        function check(condition, name) { if (!condition) throw new Error(name); }
        for (const [locale, expected] of [
            ['en-US', '1/1/1970'], ['ja-JP', '1970/1/1'],
            ['de-DE', '1.1.1970'], ['ar-EG', '١‏/١‏/١٩٧٠']
        ]) {
            const formatter = new Intl.DateTimeFormat(locale, {timeZone: 'UTC'});
            check(formatter.format(0) === expected, locale + ': ' + formatter.format(0));
            check(formatter.format === formatter.format, 'cached bound format');
            const detached = formatter.format;
            check(detached.call(null, 0) === expected, 'bound receiver');
        }
        check(Intl.DateTimeFormat('ja-JP', {timeZone:'UTC'}).format(0) === '1970/1/1', 'call without new');
        check(new Intl.DateTimeFormat('en-US', {timeZone:'UTC',month:'long',day:'numeric',year:'numeric'}).format(0) === 'January 1, 1970', 'localized month');
        const japanese = new Intl.DateTimeFormat('ja-JP-u-ca-japanese', {timeZone:'UTC',year:'numeric',month:'long',day:'numeric'}).format(0);
        check(japanese === '昭和45年1月1日', 'Japanese calendar: ' + japanese);
        const utc = new Intl.DateTimeFormat('en-US', {timeZone:'UTC'});
        const tokyo = new Intl.DateTimeFormat('en-US', {timeZone:'Asia/Tokyo'});
        check(utc.format(-3600000) === '12/31/1969', 'UTC boundary');
        check(tokyo.format(-3600000) === '1/1/1970', 'named zone boundary');
        for (const options of [{year:'short'}, {month:'wide'}, {dateStyle:'long',year:'numeric'}, {fractionalSecondDigits:0}]) {
            let threw = false;
            try { new Intl.DateTimeFormat('en-US', options); } catch (error) { threw = error instanceof RangeError || error instanceof TypeError; }
            check(threw, 'invalid component options');
        }
        for (const value of [NaN, Infinity, -Infinity, 8640000000000001]) {
            let threw = false;
            try { utc.format(value); } catch (error) { threw = error instanceof RangeError; }
            check(threw, 'invalid epoch value');
        }
        let coerced = 0;
        check(utc.format({valueOf(){ coerced++; return 0; }}) === '1/1/1970' && coerced === 1, 'input coercion');
        'native datetime contracts PASS';
    "#));
    match result {
        Ok(value) => println!("{}", value.display()),
        Err(error) => {
            let value = error.to_opaque(&mut context);
            eprintln!(
                "{}",
                value
                    .to_string(&mut context)
                    .expect("error text")
                    .to_std_string_escaped()
            );
            std::process::exit(1);
        }
    }
}
