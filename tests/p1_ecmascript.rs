//! Regression contracts for the ECMAScript builtin integration.
use omoikane::js::JsRuntime;

#[test]
fn native_datetime_fractional_seconds_use_requested_precision_and_reject_invalid_unicode_types() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => { try {
        for (const [fractionalSecondDigits, expected] of [[1,'12:00:00.9 AM'],[2,'12:00:00.98 AM'],[3,'12:00:00.987 AM']]) {
            const f = new Intl.DateTimeFormat('en-US',{
                timeZone:'UTC',hour:'numeric',minute:'numeric',second:'numeric',fractionalSecondDigits
            });
            if (f.format(987) !== expected) throw new Error('fractional output: '+f.format(987));
            if (f.resolvedOptions().fractionalSecondDigits !== fractionalSecondDigits) throw new Error('resolved digits');
        }
        for (const name of ['calendar','numberingSystem']) {
            for (const value of ['', 'ab', 'gregory-nu', 'gregoryé', 'gregory--']) {
                let rejected = false;
                try { new Intl.DateTimeFormat('en-US',{[name]:value}); }
                catch(error) { rejected = error instanceof RangeError; }
                if (!rejected) throw new Error('invalid '+name+': '+value);
            }
        }
        return 'PASS';
    } catch(error) { return String(error); } })()"#).unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "PASS");
}

#[test]
fn native_datetime_initialization_validates_zones_and_filters_supported_locales() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => { try {
        for (const timeZone of ['Mars/Olympus', '+24', '+1', '-2400', '+01:00:00', '−09']) {
            let rejected = false;
            const options = {timeZone, get year(){throw new Error('components read before zone validation');}};
            try { new Intl.DateTimeFormat('en-US', options); }
            catch(error) { rejected = error instanceof RangeError; }
            if (!rejected) throw new Error('invalid zone: '+timeZone);
        }
        for (const [input, expected] of [['+03','+03:00'],['-0232','-02:32'],['-00','+00:00'],
            ['utc','UTC'],['eTc/gMt','UTC'],['aSIA/tOKYO','Asia/Tokyo'],['asia/calcutta','Asia/Kolkata']]) {
            const actual = new Intl.DateTimeFormat('en-US',{timeZone:input}).resolvedOptions().timeZone;
            if (actual !== expected) throw new Error('zone '+input+': '+actual);
        }
        const clock = zone => new Intl.DateTimeFormat('en-US',{
            timeZone:zone,hour:'numeric',minute:'2-digit',hour12:false
        });
        if (clock('+03').format(0) !== '03:00') throw new Error('fixed offset');
        const ny = clock('America/New_York');
        if (ny.format(Date.UTC(2025,0,1,12)) !== '07:00' || ny.format(Date.UTC(2025,6,1,12)) !== '08:00') {
            throw new Error('seasonal offset');
        }
        const supported = Intl.DateTimeFormat.supportedLocalesOf(['en-US','ja-JP','de-DE','zxx'],{localeMatcher:'lookup'});
        if (supported.join(',') !== 'en-US,ja-JP,de-DE') throw new Error('supported locales: '+supported);
        if (Intl.DateTimeFormat.supportedLocalesOf.length !== 1) throw new Error('method length');
        let reads = 0;
        new Intl.DateTimeFormat('en-US',{get timeZone(){reads++;return {toString(){reads++;return 'UTC';}};}});
        if (reads !== 2) throw new Error('zone coercion count');
        return 'PASS';
    } catch(error) { return String(error); } })()"#).unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "PASS");
}

#[test]
fn native_datetime_resolved_options_report_effective_pattern_and_numbering_system() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => { try {
        const formatter = new Intl.DateTimeFormat('en-US', {timeZone:'UTC',hour:'numeric',minute:'numeric'});
        const options = formatter.resolvedOptions();
        if (options.calendar !== 'gregory' || options.numberingSystem !== 'latn' ||
            options.timeZone !== 'UTC' || options.hourCycle !== 'h12' || options.hour12 !== true ||
            options.hour !== 'numeric' || options.minute !== '2-digit' || 'dayPeriod' in options) {
            throw new Error('resolved clock: '+JSON.stringify(options));
        }
        options.minute = 'changed';
        if (formatter.resolvedOptions().minute !== '2-digit') throw new Error('fresh options');
        const arabic = new Intl.DateTimeFormat('ar-EG',{timeZone:'UTC'}).resolvedOptions();
        if (arabic.numberingSystem !== 'arab') throw new Error('default digits: '+JSON.stringify(arabic));
        const override = new Intl.DateTimeFormat('ar-EG',{timeZone:'UTC',numberingSystem:'latn'}).resolvedOptions();
        if (override.numberingSystem !== 'latn') throw new Error('requested digits');
        const japanese = new Intl.DateTimeFormat('ja-JP-u-ca-japanese',{
            timeZone:'UTC',year:'numeric',month:'long',day:'numeric'
        }).resolvedOptions();
        if (japanese.calendar !== 'japanese' || japanese.month !== 'numeric') throw new Error('resolved calendar: '+JSON.stringify(japanese));
        const style = new Intl.DateTimeFormat('en-US',{timeZone:'UTC',dateStyle:'long'}).resolvedOptions();
        if (style.dateStyle !== 'long' || 'year' in style || 'month' in style || 'day' in style ||
            'hourCycle' in style || 'hour12' in style) throw new Error('style properties');
        const descriptor = Object.getOwnPropertyDescriptor(options,'hourCycle');
        if (!descriptor.writable || !descriptor.enumerable || !descriptor.configurable) throw new Error('data property');
        let rejected = false;
        try { Intl.DateTimeFormat.prototype.resolvedOptions.call({}); }
        catch(error) { rejected = error instanceof TypeError; }
        if (!rejected) throw new Error('receiver brand');
        return 'PASS';
    } catch(error) { return String(error); } })()"#).unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "PASS");
}

#[test]
fn native_date_locale_methods_use_intrinsic_formatter_and_validate_before_options() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => { try {
        for (const [locale, expected] of [['en-US','1/1/1970'],['ja-JP','1970/1/1'],['de-DE','1.1.1970']]) {
            if (new Date(0).toLocaleDateString(locale, {timeZone:'UTC'}) !== expected) throw new Error(locale);
        }
        if (new Date(0).toLocaleTimeString('en-US',{timeZone:'UTC'}) !== '12:00:00 AM') throw new Error('clock padding: '+new Date(0).toLocaleTimeString('en-US',{timeZone:'UTC'})+' locale='+new Intl.DateTimeFormat('en-US').resolvedOptions().locale);
        if (new Date(0).toLocaleTimeString('de-DE',{timeZone:'UTC'}) !== '00:00:00') throw new Error('locale hour padding');
        for (const [locale, expected] of [['en-US','1/1/1970, 12:00:00 AM'],['ja-JP','1970/1/1 0:00:00'],['de-DE','1.1.1970, 00:00:00']]) {
            const actual = new Date(0).toLocaleString(locale, {timeZone:'UTC'});
            if (actual !== expected) throw new Error(locale+' datetime: '+actual);
        }
        const options = new Proxy({}, {get(){ throw new Error('unexpected options access'); }});
        for (const method of ['toLocaleString','toLocaleDateString','toLocaleTimeString']) {
            if (new Date(NaN)[method]('invalid_locale', options) !== 'Invalid Date') throw new Error('invalid Date');
            let rejected = false;
            try { Date.prototype[method].call({}, 'en-US', options); } catch(error) { rejected = error instanceof TypeError; }
            if (!rejected) throw new Error('receiver validation');
        }
        for (const [method, option] of [['toLocaleDateString','timeStyle'],['toLocaleTimeString','dateStyle']]) {
            let rejected = false;
            try { new Date(0)[method]('en-US', {[option]:'short'}); } catch(error) { rejected = error instanceof TypeError; }
            if (!rejected) throw new Error('incompatible style');
        }
        const calls = [];
        new Date(0).toLocaleDateString([{toString(){calls.push('locale');return 'en-US';}}],
            {get year(){calls.push('year');return 'numeric';},timeZone:'UTC'});
        if (calls.join(',') !== 'locale,year') throw new Error('locale/options evaluation order: '+calls);
        Intl.DateTimeFormat = function(){throw new Error('author constructor');};
        if (new Date(0).toLocaleDateString('ja-JP',{timeZone:'UTC'}) !== '1970/1/1') throw new Error('intrinsic formatter');
        return 'PASS';
    } catch(error) { return String(error); } })()"#).unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "PASS");
}

#[test]
fn native_datetime_format_preserves_locale_calendar_and_bound_function() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        for (const [locale, expected] of [
            ['en-US', '1/1/1970'], ['ja-JP', '1970/1/1'], ['de-DE', '1.1.1970'],
            ['ar-EG', '١‏/١‏/١٩٧٠']
        ]) {
            const formatter = new Intl.DateTimeFormat(locale, {timeZone:'UTC'});
            if (formatter.format(0) !== expected) throw new Error(locale + ': ' + formatter.format(0));
            const format = formatter.format;
            if (format !== formatter.format || format.call(null, 0) !== expected) {
                throw new Error('bound format identity/receiver');
            }
        }
        const japanese = new Intl.DateTimeFormat('ja-JP-u-ca-japanese', {
            timeZone:'UTC', year:'numeric', month:'long', day:'numeric'
        });
        if (japanese.format(0) !== '昭和45年1月1日') throw new Error('Japanese month unit');
        const tokyo = new Intl.DateTimeFormat('en-US', {timeZone:'Asia/Tokyo'});
        if (tokyo.format(-3600000) !== '1/1/1970') throw new Error('named zone boundary');
        const utc = Intl.DateTimeFormat('en-US', {timeZone:'UTC'});
        let conversions = 0;
        if (utc.format({valueOf(){ conversions++; return 0; }}) !== '1/1/1970' ||
            conversions !== 1) throw new Error('epoch conversion');
        for (const value of [NaN, Infinity, -Infinity, 8640000000000001]) {
            let rejected = false;
            try { utc.format(value); } catch (error) { rejected = error instanceof RangeError; }
            if (!rejected) throw new Error('invalid time value');
        }
        for (const options of [{year:'short'}, {month:'wide'},
            {dateStyle:'long',year:'numeric'}, {fractionalSecondDigits:0}]) {
            let rejected = false;
            try { new Intl.DateTimeFormat('en-US', options); }
            catch (error) { rejected = error instanceof TypeError || error instanceof RangeError; }
            if (!rejected) throw new Error('invalid component options');
        }
        return 'PASS';
    })()"#).unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "PASS");
}

#[test]
fn iterator_constructor_and_from_preserve_native_prototypes_and_protocol() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
        for (const create of [() => Iterator(), () => new Iterator()]) {
            let rejected = false;
            try {create();} catch (error) {rejected = error instanceof TypeError;}
            if (!rejected) throw new Error('abstract constructor');
        }
        class Derived extends Iterator {}
        if (!(new Derived() instanceof Iterator)) throw new Error('subclass');
        const array = [1, 2].values();
        if (Object.getPrototypeOf(Object.getPrototypeOf(array)) !== Iterator.prototype ||
            Iterator.from(array) !== array) throw new Error('native prototype identity');
        if (Array.from(Iterator.from('a😀')).join(',') !== 'a,😀') throw new Error('string');
        const stringIterator = String.prototype[Symbol.iterator];
        let observedType;
        Object.defineProperty(String.prototype, Symbol.iterator, {configurable: true, get() {
            'use strict'; observedType = typeof this; return stringIterator;
        }});
        Iterator.from('');
        if (observedType !== 'string') throw new Error('primitive getter receiver');
        Iterator.from(new String(''));
        if (observedType !== 'object') throw new Error('boxed getter receiver');
        let reads = 0, returns = 0;
        const original = {get next() {reads++; return function() {
            if (this !== original || arguments.length) throw new Error('next receiver');
            return 23;
        };}, get return() {returns++; return function() {
            if (this !== original || arguments.length) throw new Error('return receiver');
            return 29;
        };}};
        const wrapper = Iterator.from(original);
        if (reads !== 1 || returns !== 0 || wrapper.next('ignored') !== 23 ||
            wrapper.return('ignored') !== 29 || returns !== 1) throw new Error('capture timing');
        const emptyReturn = Iterator.from({next() {return {done: true};}}).return();
        if (!emptyReturn.done || emptyReturn.value !== undefined) throw new Error('default return');
        for (const primitive of [null, undefined, 3, true, Symbol(), 1n]) {
            let rejected = false;
            try {Iterator.from(primitive);} catch (error) {rejected = error instanceof TypeError;}
            if (!rejected) throw new Error('primitive accepted');
        }
        const descriptor = Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor');
        if (descriptor.get.call() !== Iterator) throw new Error('constructor getter');
        const child = Object.create(Iterator.prototype);
        Object.freeze(Iterator.prototype);
        child.constructor = 'child';
        if (child.constructor !== 'child' || Iterator.prototype.constructor !== Iterator) {
            throw new Error('inherited setter');
        }
        return Object.prototype.toString.call(wrapper) === '[object Iterator]';
    })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn iterator_from_keeps_captured_next_and_iterator_alive_across_gc() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.retainedIterator = (() => {
            const iterator = {value: 17, get next() {
                const captured = {value: 23};
                return function() {return {done: false, value: this.value + captured.value++};};
            }};
            return Iterator.from(iterator);
        })();
    "#,
        )
        .unwrap();
    boa_gc::force_collect();
    let result = runtime
        .eval("retainedIterator.next().value + ',' + retainedIterator.next().value")
        .unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "40,41");
}

#[test]
fn async_sync_disposer_wrapper_returns_a_promise_observed_by_await() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.asyncDisposalResult = 'pending';
        (async () => {
            const descriptor = Object.getOwnPropertyDescriptor(Promise.prototype, 'constructor');
            const observed = [];
            const stack = new AsyncDisposableStack();
            stack.use({[Symbol.dispose]() {return {get then() {throw new Error('ignored result');}};}});
            let output;
            try {
                Object.defineProperty(Promise.prototype, 'constructor', {
                    configurable: true, get() {observed.push(this); return Promise;}
                });
                output = stack.disposeAsync();
            } finally {
                Object.defineProperty(Promise.prototype, 'constructor', descriptor);
            }
            if (observed.length !== 1 || observed[0] === output || !(observed[0] instanceof Promise)) {
                throw new Error('sync wrapper promise not observed');
            }
            await output;
            asyncDisposalResult = 'passed';
        })().catch(error => {asyncDisposalResult = String(error);});
    "#).unwrap();
    runtime.run_jobs().unwrap();
    let result = runtime.eval("asyncDisposalResult").unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        "passed"
    );
}

#[test]
fn async_disposal_awaits_lifo_resources_and_ignores_sync_fallback_results() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.asyncDisposalResult = 'pending';
        (async () => {
            const events = [];
            const stack = new AsyncDisposableStack();
            const resource = {get [Symbol.dispose]() {throw new Error('fallback read');},
                async [Symbol.asyncDispose]() {
                    if (this !== resource) throw new Error('receiver');
                    events.push('use'); await Promise.resolve(); events.push('use done');
                }};
            stack.use(resource);
            stack.use({[Symbol.dispose]() {
                events.push('sync');
                return {get then() {throw new Error('sync return observed');}};
            }});
            stack.adopt(7, async function(value) {'use strict';
                if (this !== undefined || value !== 7) throw new Error('adopt');
                events.push('adopt'); await Promise.resolve(); events.push('adopt done');
            });
            stack.defer(async () => {events.push('defer'); await Promise.resolve(); events.push('defer done');});
            const moved = stack.move();
            if (!stack.disposed || moved.disposed) throw new Error('move');
            const first = moved[Symbol.asyncDispose]();
            if (!moved.disposed || events.join(',') !== 'defer') throw new Error('eager state');
            const second = moved.disposeAsync();
            if (first === second) throw new Error('promise identity');
            if (await second !== undefined || await first !== undefined) throw new Error('return');
            if (events.join(',') !== 'defer,defer done,adopt,adopt done,sync,use,use done') {
                throw new Error('order: ' + events);
            }
            let rejected = false;
            try { await AsyncDisposableStack.prototype.disposeAsync.call(new DisposableStack()); }
            catch (error) {rejected = error instanceof TypeError;}
            if (!rejected) throw new Error('cross-type receiver');
            if (AsyncDisposableStack.prototype.disposeAsync !== AsyncDisposableStack.prototype[Symbol.asyncDispose]) {
                throw new Error('symbol alias');
            }
            asyncDisposalResult = 'passed';
        })().catch(error => {asyncDisposalResult = String(error);});
    "#).unwrap();
    runtime.run_jobs().unwrap();
    let result = runtime.eval("asyncDisposalResult").unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        "passed"
    );
}

#[test]
fn async_disposal_preserves_rejections_and_uses_internal_promise_reactions() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime.eval(r#"
        globalThis.asyncDisposalResult = 'pending';
        (async () => {
            const errors = [new Error('one'), new Error('two'), new Error('three')];
            const stack = new AsyncDisposableStack();
            stack.defer(async () => {throw errors[0];});
            stack.defer(() => {throw errors[1];});
            stack.defer(() => Promise.reject(errors[2]));
            let caught;
            try { await stack.disposeAsync(); } catch (error) {caught = error;}
            if (!(caught instanceof SuppressedError) || caught.error !== errors[0] ||
                !(caught.suppressed instanceof SuppressedError) ||
                caught.suppressed.error !== errors[1] || caught.suppressed.suppressed !== errors[2]) {
                throw new Error('suppression');
            }
            const input = Promise.resolve();
            let constructorReads = 0;
            Object.defineProperty(input, 'constructor', {get() {constructorReads++; return Promise;}});
            Object.defineProperty(input, 'then', {get() {throw new Error('author then read');}});
            const awaiting = new AsyncDisposableStack();
            awaiting.defer(() => input);
            await awaiting.disposeAsync();
            if (constructorReads !== 1) throw new Error('extra constructor/species access');
            asyncDisposalResult = 'passed';
        })().catch(error => {asyncDisposalResult = String(error);});
    "#).unwrap();
    runtime.run_jobs().unwrap();
    let result = runtime.eval("asyncDisposalResult").unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        "passed"
    );
}

#[test]
fn async_disposal_continuation_retains_remaining_resources_across_gc() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.disposalLog = [];
        globalThis.pendingDisposal = Promise.withResolvers();
        globalThis.disposalPromise = (() => {
            const stack = new AsyncDisposableStack();
            stack.adopt({value: 17}, value => disposalLog.push(value.value));
            stack.use({value: 19, [Symbol.dispose]() {disposalLog.push(this.value);}});
            stack.defer(() => pendingDisposal.promise);
            return stack.disposeAsync();
        })();
    "#,
        )
        .unwrap();
    boa_gc::force_collect();
    runtime.eval("pendingDisposal.resolve();").unwrap();
    runtime.run_jobs().unwrap();
    runtime.eval("globalThis.asyncDisposalResult = 'pending'; disposalPromise.then(() => {asyncDisposalResult = disposalLog.join(',');}, error => {asyncDisposalResult = String(error);});").unwrap();
    runtime.run_jobs().unwrap();
    let result = runtime.eval("asyncDisposalResult").unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "19,17");
}

#[test]
fn moved_disposal_records_keep_values_and_callbacks_alive_across_gc() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.disposalLog = [];
        globalThis.retainedStack = (() => {
            const original = new DisposableStack();
            original.adopt({value: 17}, value => disposalLog.push(value.value));
            original.use({value: 19, [Symbol.dispose]() {disposalLog.push(this.value);}});
            const captured = {value: 23};
            original.defer(() => disposalLog.push(captured.value));
            return original.move();
        })();
    "#,
        )
        .unwrap();
    boa_gc::force_collect();
    let result = runtime
        .eval("retainedStack.dispose(); disposalLog.join(',')")
        .unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        "23,19,17"
    );
}

#[test]
fn disposable_stack_orders_resources_moves_ownership_and_suppresses_errors() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
        const calls = [];
        const stack = new DisposableStack();
        const resource = {[Symbol.dispose]() {
            if (this !== resource || !moved.disposed) throw new Error('receiver or state');
            calls.push('use');
            moved.dispose();
        }};
        if (stack.use(resource) !== resource || stack.use(null) !== null) throw new Error('use');
        if (stack.adopt(7, function(value) {'use strict';
            if (this !== undefined || value !== 7) throw new Error('adopt arguments');
            calls.push('adopt');
        }) !== 7) throw new Error('adopt return');
        stack.defer(function() {'use strict';
            if (this !== undefined || arguments.length !== 0) throw new Error('defer arguments');
            calls.push('defer');
        });
        const moved = stack.move();
        if (!stack.disposed || moved.disposed || moved === stack) throw new Error('move state');
        stack.dispose();
        if (calls.length !== 0) throw new Error('moved ownership');
        moved[Symbol.dispose]();
        moved.dispose();
        if (calls.join(',') !== 'defer,adopt,use') throw new Error('LIFO');
        for (const operation of [() => stack.use(null), () => stack.adopt(0, null),
                                 () => stack.defer(null), () => stack.move()]) {
            let rejected = false;
            try { operation(); } catch (error) { rejected = error instanceof ReferenceError; }
            if (!rejected) throw new Error('disposed stack accepted');
        }
        const errors = [new Error('first'), new Error('second'), new Error('third')];
        const failures = new DisposableStack();
        for (const error of errors) failures.defer(() => {throw error;});
        let caught;
        try { failures.dispose(); } catch (error) {caught = error;}
        if (!(caught instanceof SuppressedError) || caught.error !== errors[0] ||
            !(caught.suppressed instanceof SuppressedError) ||
            caught.suppressed.error !== errors[1] || caught.suppressed.suppressed !== errors[2]) {
            throw new Error('suppressed errors');
        }
        class Derived extends DisposableStack {}
        if (!(new Derived() instanceof Derived)) throw new Error('subclass');
        return DisposableStack.prototype.dispose === DisposableStack.prototype[Symbol.dispose] &&
            Object.prototype.toString.call(moved) === '[object DisposableStack]';
    })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn suppressed_error_preserves_error_values_and_native_error_brand() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        const first = new Error('first'), second = new Error('second');
        const error = SuppressedError(first, second, 'disposal');
        if (!(error instanceof Error) || !(error instanceof SuppressedError) || !Error.isError(error)) {
            throw new Error('error brand');
        }
        if (error.error !== first || error.suppressed !== second ||
            error.message !== 'disposal' || String(error) !== 'SuppressedError: disposal') {
            throw new Error('error fields');
        }
        for (const key of ['error', 'suppressed', 'message']) {
            const descriptor = Object.getOwnPropertyDescriptor(error, key);
            if (descriptor.enumerable || !descriptor.writable || !descriptor.configurable) {
                throw new Error('descriptor');
            }
        }
        class Derived extends SuppressedError {}
        if (!(new Derived(first, second) instanceof Derived)) throw new Error('subclass');
        let causeRead = false;
        const plain = new SuppressedError(undefined, undefined, undefined,
            {get cause() {causeRead = true;}});
        return SuppressedError.length === 3 && !causeRead &&
            !Object.hasOwn(plain, 'message') && Object.hasOwn(plain, 'error') &&
            Object.hasOwn(plain, 'suppressed');
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn disposal_symbols_are_native_well_known_symbols_shared_between_realms() {
    let mut first = JsRuntime::new().unwrap();
    let mut second = JsRuntime::new().unwrap();
    for name in ["dispose", "asyncDispose"] {
        let script = format!("Symbol.{name}");
        let a = first.eval(&script).unwrap();
        let b = second.eval(&script).unwrap();
        assert!(a.is_symbol());
        assert_eq!(a.as_symbol(), b.as_symbol());
    }
    let result = first
        .eval(
            r#"(() => {
        if (Symbol.dispose === Symbol.asyncDispose) throw new Error('distinct symbols');
        for (const name of ['dispose', 'asyncDispose']) {
            const symbol = Symbol[name];
            if (symbol.description !== 'Symbol.' + name) throw new Error('description');
            if (Symbol.keyFor(symbol) !== undefined) throw new Error('registry');
            if (symbol === Symbol.for('Symbol.' + name)) throw new Error('registry identity');
            const descriptor = Object.getOwnPropertyDescriptor(Symbol, name);
            if (descriptor.value !== symbol || descriptor.writable || descriptor.enumerable ||
                descriptor.configurable) throw new Error('descriptor');
            const object = {[symbol]() {}};
            if (object[symbol].name !== '[Symbol.' + name + ']') throw new Error('function name');
        }
        return true;
    })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn temporal_time_overflow_retains_signed_fields_until_options_are_read() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        const time = new Temporal.PlainTime();
        const limits = {hour: 23, minute: 59, second: 59,
                        millisecond: 999, microsecond: 999, nanosecond: 999};
        for (const [field, limit] of Object.entries(limits)) {
            for (const value of [-1, limit + 1, Number.MAX_VALUE]) {
                for (const operation of [x => time.with(x, {overflow:'reject'}),
                                        x => Temporal.PlainTime.from(x, {overflow:'reject'})]) {
                    let rejected = false;
                    try { operation({[field]: value}); }
                    catch (error) { rejected = error instanceof RangeError; }
                    if (!rejected) throw new Error('accepted: ' + field + '=' + value);
                }
            }
            if (time.with({[field]: -1})[field] !== 0) throw new Error('lower constrain');
            if (time.with({[field]: limit + 1})[field] !== limit) throw new Error('upper constrain');
            if (time.with({[field]: -0.9}, {overflow:'reject'})[field] !== 0) {
                throw new Error('truncation before range check');
            }
        }
        const events = [];
        const fields = new Proxy({hour: -1}, {get(target, key) {
            if (key === 'hour') events.push('hour');
            return Reflect.get(target, key);
        }});
        const sentinel = new Error('options');
        let observed = false;
        try { time.with(fields, {get overflow() {events.push('overflow'); throw sentinel;}}); }
        catch (error) { observed = error === sentinel; }
        return observed && events.join(',') === 'hour,overflow';
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn native_temporal_and_error_branding_are_available_in_browser_runtime() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
        const date = Temporal.PlainDate.from('2024-02-28').add({days: 2});
        if (date.toString() !== '2024-03-01') throw new Error('calendar arithmetic');
        const instant = Temporal.Instant.from('1970-01-01T00:00:00Z');
        if (instant.epochNanoseconds !== 0n) throw new Error('instant');
        const error = new TypeError('native error');
        if (!Error.isError(error) || Error.isError(new Proxy(error, {}))) {
            throw new Error('error branding');
        }
        return !Error.isError({[Symbol.toStringTag]: 'Error'});
    })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn temporal_iso_parsing_rejects_excess_fraction_digits_without_truncation() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
        const parsers = [
            [Temporal.Instant, '1970-01-01T00:00:00.123456789Z'],
            [Temporal.PlainDate, '1970-01-01T00:00:00.123456789'],
            [Temporal.PlainDateTime, '1970-01-01T00:00:00.123456789'],
            [Temporal.PlainTime, '00:00:00.123456789'],
            [Temporal.PlainYearMonth, '1970-01-01T00:00:00.123456789'],
            [Temporal.PlainMonthDay, '1970-01-01T00:00:00.123456789'],
            [Temporal.ZonedDateTime, '1970-01-01T00:00:00.123456789Z[UTC]']
        ];
        for (const [type, valid] of parsers) {
            type.from(valid);
            type.from(valid.replace('.', ','));
            for (const tail of ['0', '1']) {
                let rejected = false;
                try { type.from(valid.replace('123456789', '123456789' + tail)); }
                catch (error) { rejected = error instanceof RangeError; }
                if (!rejected) throw new Error('excess precision: ' + type.name);
            }
        }
        for (const offset of ['1970-01-01T00+00:00:00.1234567890',
                              '1970-01-01T00+00:00:00.1234567891']) {
            let rejected = false;
            try { Temporal.Instant.from(offset); }
            catch (error) { rejected = error instanceof RangeError; }
            if (!rejected) throw new Error('offset precision');
        }
        return true;
    })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn array_from_async_awaits_values_and_closes_on_mapping_failure() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.asyncBuiltinResult = 'pending';
        (async () => {
            const values = await Array.fromAsync([Promise.resolve(2), 3], async x => x * 2);
            if (values.join(',') !== '4,6') throw new Error('awaited mapping');
            let closed = 0;
            const sentinel = new Error('mapping failure');
            const iterable = {
                [Symbol.asyncIterator]() { return this; },
                async next() { return {value: 1, done: false}; },
                async return() { closed++; return {done: true}; }
            };
            let rejected = false;
            try { await Array.fromAsync(iterable, () => { throw sentinel; }); }
            catch (error) { rejected = error === sentinel; }
            if (!rejected || closed !== 1) throw new Error('iterator cleanup');
            asyncBuiltinResult = 'passed';
        })().catch(error => { asyncBuiltinResult = String(error); });
    "#,
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    let result = runtime.eval("asyncBuiltinResult").unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        "passed"
    );
}

#[test]
fn number_locale_formatting_uses_native_intrinsics() {
    let mut runtime = JsRuntime::new().unwrap();
    for (script, expected) in [
        ("(1234.5).toLocaleString('en-US')", "1,234.5"),
        ("(1234.5).toLocaleString('ja-JP')", "1,234.5"),
        ("(1234.5).toLocaleString('de-DE')", "1.234,5"),
        (
            "(1.2).toLocaleString('en-US', {minimumFractionDigits:3})",
            "1.200",
        ),
        (
            "Number.prototype.toLocaleString.call(new Number(1234.5), 'de-DE')",
            "1.234,5",
        ),
    ] {
        let result = runtime.eval(script).unwrap();
        assert_eq!(
            result.as_string().unwrap().to_std_string_escaped(),
            expected
        );
    }
    runtime
        .eval("Intl.NumberFormat = () => { throw new Error('author override'); }")
        .unwrap();
    let result = runtime.eval("(1234.5).toLocaleString('de-DE')").unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        "1.234,5"
    );
    assert!(
        runtime
            .eval("(1).toLocaleString('invalid_locale')")
            .is_err()
    );
    assert!(
        runtime
            .eval("Number.prototype.toLocaleString.call({}, 'en-US')")
            .is_err()
    );
}

#[test]
fn regexp_escape_preserves_literals_and_rejects_coercion() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        const cases = [['foo', '\\x66oo'], ['1abc', '\\x31abc'],
            ['a-b', '\\x61\\x2db'], ['.', '\\.'],
            [' ', '\\x20'], ['\n', '\\n'],
            ['\u2028', '\\u2028'], ['\ud800', '\\ud800'],
            ['😀', '😀'], ['', '']];
        for (const [input, expected] of cases) {
            if (RegExp.escape(input) !== expected) throw new Error(input);
            if (!new RegExp('^' + RegExp.escape(input) + '$', 'u').test(input)) throw new Error('roundtrip');
        }
        let calls = 0;
        for (const input of [undefined, 1, new String('a'), {toString(){calls++; return 'a'}}]) {
            let rejected = false;
            try { RegExp.escape(input); } catch (error) { rejected = error instanceof TypeError; }
            if (!rejected) throw new Error('non-string accepted');
        }
        return calls === 0 && RegExp.escape.length === 1;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn uint8_hex_respects_view_bounds_and_receiver_type() {
    let mut runtime = JsRuntime::new().unwrap();
    for (source, expected) in [
        (
            "new Uint8Array([0, 1, 15, 16, 127, 128, 255]).toHex()",
            "00010f107f80ff",
        ),
        (
            "new Uint8Array([1, 2, 3, 4]).subarray(1, 3).toHex()",
            "0203",
        ),
        ("new Uint8Array().toHex()", ""),
    ] {
        let result = runtime.eval(source).unwrap();
        assert_eq!(
            result.as_string().unwrap().to_std_string_escaped(),
            expected
        );
    }
    assert!(
        runtime
            .eval("Uint8Array.prototype.toHex.call(new Int8Array(1))")
            .is_err()
    );
    assert!(
        runtime
            .eval("Uint8Array.prototype.toHex.call(new Uint8ClampedArray(1))")
            .is_err()
    );
    assert!(
        runtime
            .eval("Uint8Array.prototype.toHex.call(new Proxy(new Uint8Array(1), {}))")
            .is_err()
    );
    assert!(
        runtime
            .eval(
                "const detached = new Uint8Array(1); detached.buffer.transfer(); detached.toHex()"
            )
            .is_err()
    );
}

#[test]
fn uint8_from_hex_is_strict_and_uses_intrinsic_constructor() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        if (Uint8Array.fromHex('000f10aBFF').toHex() !== '000f10abff') throw new Error('bytes');
        if (Uint8Array.fromHex('').length !== 0) throw new Error('empty');
        if (Uint8Array.fromHex.call(null, 'ff')[0] !== 255) throw new Error('this');
        class Subclass extends Uint8Array {}
        if (Subclass.fromHex('ff') instanceof Subclass) throw new Error('subclass');
        for (const input of ['f', 'fg', ' f', 'ff ', '\ud800x', '😀']) {
            let rejected = false;
            try { Uint8Array.fromHex(input); } catch (error) { rejected = error instanceof SyntaxError; }
            if (!rejected) throw new Error('syntax');
        }
        for (const input of [1, new String('ff'), undefined]) {
            let rejected = false;
            try { Uint8Array.fromHex(input); } catch (error) { rejected = error instanceof TypeError; }
            if (!rejected) throw new Error('type');
        }
        return true;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn uint8_set_from_hex_preserves_partial_writes() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        const bytes = new Uint8Array([9,9,9,9]);
        const view = bytes.subarray(1,3);
        const result = view.setFromHex('abcdxx');
        if (result.read !== 4 || result.written !== 2 || bytes.toHex() !== '09abcd09') throw new Error('bounds');
        let rejected = false;
        try { bytes.setFromHex('0011xx'); } catch (error) {
            rejected = error instanceof SyntaxError;
        }
        if (!rejected) throw new Error('invalid digits accepted');
        if (bytes.toHex() !== '0011cd09') throw new Error('partial');
        rejected = false;
        try { bytes.setFromHex('123'); } catch (error) {
            rejected = error instanceof SyntaxError;
        }
        if (!rejected) throw new Error('odd input accepted');
        if (bytes.toHex() !== '0011cd09') throw new Error('odd mutated');
        const empty = new Uint8Array();
        if (empty.setFromHex('xx').read !== 0) throw new Error('empty');
        return true;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn uint8_base64_encodes_options_and_checks_buffer_after_getters() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        for (const [hex, expected] of [['',''], ['66','Zg=='], ['666f','Zm8='], ['666f6f','Zm9v'], ['fbff','+/8=']]) {
            if (Uint8Array.fromHex(hex).toBase64() !== expected) throw new Error('encoding');
        }
        if (Uint8Array.fromHex('fbff').toBase64({alphabet:'base64url',omitPadding:true}) !== '-_8') throw new Error('url');
        for (const options of [null, 1, {alphabet:'invalid'}, {alphabet:{toString(){return 'base64'}}}]) {
            let rejected = false;
            try { new Uint8Array().toBase64(options); } catch (error) { rejected = error instanceof TypeError; }
            if (!rejected) throw new Error('options');
        }
        const bytes = new Uint8Array([1]);
        let order = '';
        let rejected = false;
        try { bytes.toBase64({get alphabet(){order+='a'; bytes.buffer.transfer(); return 'base64'},
                              get omitPadding(){order+='p'; return false}}); }
        catch(error) { rejected = error instanceof TypeError; }
        if (!rejected || order !== 'ap') throw new Error('detach order');
        return true;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn uint8_base64_decoding_modes_and_partial_writes() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        if (Uint8Array.fromBase64(' Z m9v\n').toHex() !== '666f6f') throw new Error('whitespace');
        if (Uint8Array.fromBase64('-_8=', {alphabet:'base64url'}).toHex() !== 'fbff') throw new Error('url');
        if (Uint8Array.fromBase64('Zg').toHex() !== '66') throw new Error('loose');
        if (Uint8Array.fromBase64('Zm9vZg', {lastChunkHandling:'stop-before-partial'}).toHex() !== '666f6f') throw new Error('stop');
        for (const input of ['Zg', 'Zh==', 'Zm9v!']) {
            let rejected = false;
            try { Uint8Array.fromBase64(input, {lastChunkHandling:'strict'}); }
            catch(error) { rejected = error instanceof SyntaxError; }
            if (!rejected) throw new Error('strict');
        }
        const bytes = new Uint8Array(6);
        let rejected = false;
        try { bytes.setFromBase64('Zm9v!'); } catch(error) { rejected = error instanceof SyntaxError; }
        if (!rejected || bytes.toHex() !== '666f6f000000') throw new Error('partial');
        const tiny = new Uint8Array(1);
        if (tiny.setFromBase64('Zg==').written !== 1 || tiny[0] !== 102) throw new Error('bounded');
        const result = new Uint8Array(2).setFromBase64('Zm9v');
        return result.read === 0 && result.written === 0;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn uint8_base64_validation_order_and_intrinsic_allocation() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime
        .eval(
            r#"(() => {
        class Subclass extends Uint8Array {}
        if (Subclass.fromBase64('Zg==') instanceof Subclass) throw new Error('subclass');
        if (Uint8Array.fromBase64.call(null, 'Zg==')[0] !== 102) throw new Error('this');
        let calls = 0;
        const options = {get alphabet(){calls++; return 'base64'}};
        for (const input of [undefined, 1, new String('Zg==')]) {
            let rejected = false;
            try { Uint8Array.fromBase64(input, options); }
            catch(error) { rejected = error instanceof TypeError; }
            if (!rejected) throw new Error('input');
        }
        if (calls !== 0) throw new Error('options before input');
        let rejected = false;
        try { Uint8Array.prototype.setFromBase64.call(new Int8Array(1), 'Zg==', options); }
        catch(error) { rejected = error instanceof TypeError; }
        if (!rejected || calls !== 0) throw new Error('receiver before options');
        const bytes = new Uint8Array(1);
        let order = '';
        rejected = false;
        try { bytes.setFromBase64('Zg==', {
            get alphabet(){order+='a'; bytes.buffer.transfer(); return 'base64'},
            get lastChunkHandling(){order+='h'; return 'loose'}
        }); } catch(error) { rejected = error instanceof TypeError; }
        if (!rejected || order !== 'ah') throw new Error('detach order');
        for (const input of [null, 1, {alphabet:'other'}, {lastChunkHandling:'other'}]) {
            rejected = false;
            try { Uint8Array.fromBase64('', input); }
            catch(error) { rejected = error instanceof TypeError; }
            if (!rejected) throw new Error('options');
        }
        return true;
    })()"#,
        )
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn iterator_collection_and_disposal_preserve_protocol_and_gc_roots() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"
        globalThis.retainedIterator = Iterator.from((function* () {
            yield {value: 7}; yield {value: 11};
        })());
        globalThis.collectedIteratorValues = retainedIterator.toArray();
        let nextGets = 0, nextCalls = 0;
        const source = {get next() {nextGets++; return function() {
            return {value: ++nextCalls, done: nextCalls > 2};
        };}};
        const values = Iterator.prototype.toArray.call(source);
        if (nextGets !== 1 || nextCalls !== 3 || values.join(',') !== '1,2') {
            throw new Error('captured next');
        }
        let calls = 0;
        const disposable = {return() {
            if (this !== disposable || arguments.length) throw new Error('return receiver');
            calls++; return 23;
        }};
        if (Iterator.prototype[Symbol.dispose].call(disposable) !== undefined || calls !== 1) {
            throw new Error('dispose result');
        }
        const failure = new Error('next failure');
        const throwing = {next() {throw failure;}, return() {throw new Error('must not close');}};
        try {Iterator.prototype.toArray.call(throwing); throw new Error('must throw');}
        catch (error) {if (error !== failure) throw error;}
    "#,
        )
        .unwrap();
    boa_gc::force_collect();
    let result = runtime
        .eval("collectedIteratorValues.map(value => value.value).join(',')")
        .unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "7,11");
}

#[test]
fn iterator_callback_consumers_close_on_callback_failure_and_short_circuit() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        const visits = [];
        [4, 8].values().forEach(function(value, index) {
            'use strict';
            if (this !== undefined || arguments.length !== 2) throw new Error('callback protocol');
            visits.push(value + index);
        });
        if (visits.join(',') !== '4,9') throw new Error('indices');
        if (![1, 2, 3].values().some(value => value === 2) ||
            [1, 2].values().every(value => value === 1) ||
            [1, 2, 3].values().find(value => value === 2) !== 2 ||
            [].values().find(() => true) !== undefined ||
            [].values().some(() => true) || ![].values().every(() => false)) {
            throw new Error('consumer results');
        }
        for (const name of ['forEach', 'some', 'every', 'find']) {
            let closed = 0;
            const invalid = {get next() {throw new Error('must not read next');},
                return() {closed++; return {};}};
            try {Iterator.prototype[name].call(invalid, null); throw new Error('must throw');}
            catch (error) {if (!(error instanceof TypeError)) throw error;}
            if (closed !== 1) throw new Error('validation close');
            const failure = {};
            const throwing = {next() {return {value: 7, done: false};},
                return() {closed++; throw new Error('secondary');}};
            try {Iterator.prototype[name].call(throwing, () => {throw failure;});}
            catch (error) {if (error !== failure) throw error;}
            if (closed !== 2) throw new Error('callback close');
            const badNext = {next() {throw failure;}, return() {closed++; return {};}};
            try {Iterator.prototype[name].call(badNext, () => true);}
            catch (error) {if (error !== failure) throw error;}
            if (closed !== 2) throw new Error('next must not close');
        }
        let finalized = 0;
        function* source() {try {yield 1; yield 2; yield 3;} finally {finalized++;}}
        if (!source().some(value => value === 2) || finalized !== 1) throw new Error('some close');
        if (source().every(value => value < 2) || finalized !== 2) throw new Error('every close');
        if (source().find(value => value === 2) !== 2 || finalized !== 3) throw new Error('find close');
        return true;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn iterator_reduce_handles_initial_values_indices_and_error_close() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        let indices = [];
        const sum = [2, 3, 5].values().reduce(function(accumulator, value, index) {
            'use strict';
            if (this !== undefined || arguments.length !== 3) throw new Error('reducer protocol');
            indices.push(index); return accumulator + value;
        });
        if (sum !== 10 || indices.join(',') !== '1,2') throw new Error('implicit initial');
        indices = [];
        const object = [2, 3].values().reduce((accumulator, value, index) => {
            indices.push(index); return {value: accumulator.value + value};
        }, {value: 7});
        if (object.value !== 12 || indices.join(',') !== '0,1') throw new Error('explicit initial');
        if ([].values().reduce(() => {throw new Error('must not call');}, undefined) !== undefined) {
            throw new Error('explicit undefined');
        }
        let emptyRejected = false;
        try {[].values().reduce(() => 1);} catch (error) {emptyRejected = error instanceof TypeError;}
        if (!emptyRejected) throw new Error('empty iterator');
        let closed = 0;
        const invalid = {get next() {throw new Error('must not read next');},
            return() {closed++; return {};}};
        try {Iterator.prototype.reduce.call(invalid, {});} catch (error) {
            if (!(error instanceof TypeError)) throw error;
        }
        if (closed !== 1) throw new Error('validation close');
        const failure = {};
        const throwing = {next() {return {value: 1, done: false};},
            return() {closed++; throw new Error('secondary');}};
        try {Iterator.prototype.reduce.call(throwing, () => {throw failure;}, 0);}
        catch (error) {if (error !== failure) throw error;}
        if (closed !== 2) throw new Error('reducer close');
        return true;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn iterator_map_filter_are_lazy_and_close_with_reentry_checks() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        let visits = [];
        const helper = [2, 3, 4].values().map(function(value, index) {
            'use strict';
            if (this !== undefined || arguments.length !== 2) throw new Error('mapper protocol');
            visits.push(index); return value * 2;
        }).filter((value, index) => value > 4 && index > 0);
        if (visits.length) throw new Error('must be lazy');
        if (helper.toArray().join(',') !== '6,8' || visits.join(',') !== '0,1,2') {
            throw new Error('lazy results');
        }
        if (Object.prototype.toString.call(helper) !== '[object Iterator Helper]') throw new Error('brand');
        if (!helper.next().done || !helper.return().done) throw new Error('completed state');
        for (const method of ['map', 'filter']) {
            let calls = 0;
            const source = {next() {calls++; return {value: 7, done: false};},
                return() {calls += 10; return {};}};
            const suspended = Iterator.prototype[method].call(source, () => true);
            suspended.return(); suspended.return();
            if (calls !== 10) throw new Error('suspended close');
            const failure = {};
            const throwing = Iterator.prototype[method].call(source, () => {throw failure;});
            try {throwing.next();} catch (error) {if (error !== failure) throw error;}
            if (calls !== 21 || !throwing.next().done) throw new Error('callback close');
            let recursive;
            recursive = [1].values()[method](() => recursive.next());
            let rejected = false;
            try {recursive.next();} catch (error) {rejected = error instanceof TypeError;}
            if (!rejected || !recursive.next().done) throw new Error('reentry');
            const invalid = {get next() {throw new Error('must not read next');},
                return() {calls++; return {};}};
            try {Iterator.prototype[method].call(invalid, null);} catch (error) {
                if (!(error instanceof TypeError)) throw error;
            }
            if (calls !== 22) throw new Error('validation close');
        }
        return true;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn iterator_take_drop_validate_before_next_and_close_at_take_boundary() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        if ([1,2,3,4].values().drop(1.9).take(2.9).toArray().join(',') !== '2,3' ||
            [1,2].values().take(Infinity).toArray().join(',') !== '1,2' ||
            [1,2].values().drop(Infinity).toArray().length !== 0 ||
            [1,2].values().take(-0.5).toArray().length !== 0) throw new Error('count results');
        let reads = 0, closed = 0;
        const source = {get next() {reads++; return () => ({value: 7, done: false});},
            return() {closed++; return {};}};
        const taken = Iterator.prototype.take.call(source, 1);
        if (reads !== 1 || closed !== 0) throw new Error('lazy take');
        if (taken.next().value !== 7 || closed !== 0 || !taken.next().done || closed !== 1) {
            throw new Error('take boundary close');
        }
        taken.return();
        if (closed !== 1) throw new Error('completed close');
        for (const method of ['take','drop']) {
            for (const limit of [undefined, NaN, -1, Number.MAX_SAFE_INTEGER + 1]) {
                let nextReads = 0, returns = 0;
                const invalid = {get next() {nextReads++; throw new Error('must not read');},
                    return() {returns++; return {};}};
                let rejected = false;
                try {Iterator.prototype[method].call(invalid, limit);}
                catch (error) {rejected = error instanceof RangeError;}
                if (!rejected || nextReads !== 0 || returns !== 1) throw new Error('validation close');
            }
        }
        return true;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn iterator_helper_callbacks_and_reduction_keep_owned_values_alive_during_gc() {
    use boa_engine::{Context, JsValue, NativeFunction, Source, js_string};
    let mut context = Context::default();
    context
        .register_global_callable(
            js_string!("forceCollect"),
            0,
            NativeFunction::from_fn_ptr(|_, _, _| {
                boa_gc::force_collect();
                Ok(JsValue::undefined())
            }),
        )
        .unwrap();
    let result = context
        .eval(Source::from_bytes(
            r#"(() => {
        const helper = (() => {
            const seed = {offset: 5};
            return [{value: 7}, {value: 11}, {value: 13}].values()
                .map(value => {forceCollect(); return {value: value.value + seed.offset};})
                .filter(value => {forceCollect(); return value.value > 12;})
                .take(2);
        })();
        forceCollect();
        const collected = helper.toArray();
        forceCollect();
        if (collected.map(value => value.value).join(',') !== '16,18') {
            throw new Error('owned helper values');
        }
        let index = 0;
        const iterated = Iterator.from({next() {
            forceCollect();
            return {done: index === 3, value: ++index};
        }});
        const reduced = iterated.reduce((accumulator, value) => {
            forceCollect(); return {value: accumulator.value + value};
        }, {value: 5});
        forceCollect();
        if (reduced.value !== 11) throw new Error('owned accumulator');
        const flat = [7].values().flatMap(value => {
            const captured = {value};
            let count = 0;
            return {next() {
                forceCollect();
                return {done: count === 2, value: {value: captured.value + count++}};
            }};
        });
        const first = flat.next().value;
        forceCollect();
        const second = flat.next().value;
        forceCollect();
        if (first.value !== 7 || second.value !== 8 || !flat.next().done) {
            throw new Error('owned flatMap inner iterator');
        }
        return true;
    })()"#,
        ))
        .unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn iterator_flat_map_is_lazy_and_closes_inner_before_outer() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        const indices = [];
        const flattened = [2, 3].values().flatMap((value, index) => {
            indices.push(index); return [value, value + 1];
        });
        if (indices.length || flattened.toArray().join(',') !== '2,3,3,4' ||
            indices.join(',') !== '0,1') throw new Error('flatMap results');
        const events = [];
        const outer = {next() {return {value: 7, done: false};},
            return() {events.push('outer'); return {};}};
        const inner = {next() {return {value: 11, done: false};},
            return() {events.push('inner'); return {};}};
        const helper = Iterator.prototype.flatMap.call(outer, () => inner);
        if (helper.next().value !== 11) throw new Error('inner value');
        helper.return(); helper.return();
        if (events.join(',') !== 'inner,outer') throw new Error('close order');
        const failure = {};
        const throwingInner = {next() {throw failure;}, return() {throw new Error('must not close');}};
        const throwing = Iterator.prototype.flatMap.call(outer, () => throwingInner);
        try {throwing.next();} catch (error) {if (error !== failure) throw error;}
        if (events.join(',') !== 'inner,outer,outer' || !throwing.next().done) {
            throw new Error('inner failure closes outer');
        }
        let rejected = false;
        try {[1].values().flatMap(() => 'abc').next();}
        catch (error) {rejected = error instanceof TypeError;}
        if (!rejected) throw new Error('primitive must not flatten');
        return true;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn finalization_registry_registers_weak_targets_and_unregisters_all_token_cells() {
    let mut runtime = JsRuntime::new().unwrap();
    let result = runtime.eval(r#"(() => {
        const registry = new FinalizationRegistry(() => {});
        const target = {}, token = {};
        if (registry.register(target, 'a', token) !== undefined ||
            registry.register({}, 'b', token) !== undefined ||
            registry.unregister(token) !== true || registry.unregister(token) !== false) {
            throw new Error('registration protocol');
        }
        registry.register(Symbol('target'), {value: 7});
        for (const invalid of [undefined, null, true, 1, 'x', 1n, Symbol.for('registered')]) {
            let rejected = false;
            try {registry.register(invalid, 'held');} catch (error) {rejected = error instanceof TypeError;}
            if (!rejected) throw new Error('invalid weak target');
        }
        let identicalRejected = false;
        try {registry.register(target, target);} catch (error) {identicalRejected = error instanceof TypeError;}
        if (!identicalRejected) throw new Error('holding identity');
        class Derived extends FinalizationRegistry {}
        if (!(new Derived(() => {}) instanceof FinalizationRegistry) ||
            Object.prototype.toString.call(registry) !== '[object FinalizationRegistry]') {
            throw new Error('prototype');
        }
        return true;
    })()"#).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn finalization_registry_cleans_collected_objects_and_symbols_only_during_jobs() {
    use boa_engine::{Context, Source};
    let mut context = Context::default();
    context.eval(Source::from_bytes(r#"
        globalThis.cleaned = [];
        globalThis.registry = new FinalizationRegistry(function(holding) {
            'use strict';
            if (this !== undefined || arguments.length !== 1) throw new Error('cleanup protocol');
            cleaned.push(holding.value);
        });
        globalThis.liveTarget = {};
        registry.register(liveTarget, {value: 'live'});
        (() => {registry.register({}, {value: 'object'}); registry.register(Symbol(), {value: 'symbol'});})();
    "#)).unwrap();
    context.clear_kept_objects();
    boa_gc::force_collect();
    assert_eq!(
        context
            .eval(Source::from_bytes("cleaned.length"))
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    context.run_jobs().unwrap();
    let result = context
        .eval(Source::from_bytes("cleaned.slice().sort().join(',')"))
        .unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        "object,symbol"
    );
    context.run_jobs().unwrap();
    assert_eq!(
        context
            .eval(Source::from_bytes("cleaned.length"))
            .unwrap()
            .as_number(),
        Some(2.0)
    );
    context
        .eval(Source::from_bytes("liveTarget = null"))
        .unwrap();
    context.clear_kept_objects();
    boa_gc::force_collect();
    context.run_jobs().unwrap();
    let result = context
        .eval(Source::from_bytes("cleaned.slice().sort().join(',')"))
        .unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        "live,object,symbol"
    );
}

#[test]
fn finalization_registry_cleanup_can_unregister_pending_cells_and_preserves_weak_tokens() {
    use boa_engine::{Context, Source};
    let mut context = Context::default();
    context
        .eval(Source::from_bytes(
            r#"
        globalThis.cleaned = [];
        globalThis.token = {};
        globalThis.registry = new FinalizationRegistry(holding => {
            cleaned.push(holding);
            if (!registry.unregister(token)) throw new Error('pending cell unregister');
        });
        (() => {registry.register({}, 'first'); registry.register({}, 'second', token);})();
        globalThis.live = {};
        globalThis.other = new FinalizationRegistry(() => {});
        (() => {const ephemeralToken = {}; globalThis.weakToken = new WeakRef(ephemeralToken);
            other.register(live, 'held', ephemeralToken);})();
    "#,
        ))
        .unwrap();
    context.clear_kept_objects();
    boa_gc::force_collect();
    assert!(
        context
            .eval(Source::from_bytes("weakToken.deref()"))
            .unwrap()
            .is_undefined()
    );
    context.run_jobs().unwrap();
    let result = context
        .eval(Source::from_bytes("cleaned.join(',')"))
        .unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "first");
}

#[test]
fn finalization_registry_reschedules_cleanup_discarded_after_another_job_fails() {
    use boa_engine::{Context, Source};
    let mut context = Context::default();
    context.eval(Source::from_bytes(r#"
        globalThis.cleaned = [];
        globalThis.failure = new Error('cleanup failure');
        globalThis.first = new FinalizationRegistry(holding => {cleaned.push(holding); throw failure;});
        globalThis.second = new FinalizationRegistry(holding => {cleaned.push(holding);});
        (() => {first.register({}, 'first'); second.register({}, 'second');})();
    "#)).unwrap();
    context.clear_kept_objects();
    boa_gc::force_collect();
    assert!(context.run_jobs().is_err());
    context.run_jobs().unwrap();
    let result = context
        .eval(Source::from_bytes("cleaned.join(',')"))
        .unwrap();
    assert_eq!(
        result.as_string().unwrap().to_std_string_escaped(),
        "first,second"
    );
}

#[test]
fn finalization_registry_preserves_creation_realm_after_registering_in_another_context() {
    use boa_engine::{Context, Source, js_string, property::Attribute};
    let mut owner = Context::default();
    owner
        .eval(Source::from_bytes(
            r#"
        globalThis.collector = {values: []};
        globalThis.registry = new FinalizationRegistry(holding => collector.values.push(holding));
    "#,
        ))
        .unwrap();
    let registry = owner.eval(Source::from_bytes("registry")).unwrap();
    let collector = owner.eval(Source::from_bytes("collector")).unwrap();
    let mut destination = Context::default();
    destination
        .register_global_property(js_string!("registry"), registry, Attribute::all())
        .unwrap();
    destination
        .register_global_property(js_string!("collector"), collector, Attribute::all())
        .unwrap();
    drop(owner);
    destination
        .eval(Source::from_bytes("(() => registry.register({}, 17))()"))
        .unwrap();
    destination.clear_kept_objects();
    boa_gc::force_collect();
    destination.run_jobs().unwrap();
    let result = destination
        .eval(Source::from_bytes("collector.values.join(',')"))
        .unwrap();
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "17");
}
