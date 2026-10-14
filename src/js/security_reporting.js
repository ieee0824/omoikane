// Reports are WebIDL dictionaries. Observer queues and the report buffer are
// private to this Window/Worker realm; each delivery gets an independent copy.
(() => {
  const get = Function.prototype.call.bind(WeakMap.prototype.get);
  const set = Function.prototype.call.bind(WeakMap.prototype.set);
  const apply = Reflect.apply, TypeErrorIntrinsic = TypeError;
  const state = new WeakMap(), registered = new Set(), buffer = [];
  const dictionary = value => {
    if (value == null) return {};
    if (typeof value !== "object" && typeof value !== "function") throw new TypeErrorIntrinsic("Expected a dictionary");
    return value;
  };
  const string = value => {
    if (typeof value === "symbol") throw new TypeErrorIntrinsic("Cannot convert a Symbol to a string");
    return String(value);
  };
  const copy = report => {
    const result = { type: report.type, url: report.url, body: report.body === null ? null : { ...report.body } };
    // Keep the existing web serialization convenience alongside the current
    // dictionary representation. It does not add a serialized member.
    Object.defineProperty(result, "toJSON", {
      configurable: true, writable: true,
      value() { return { type: this.type, url: this.url, body: this.body }; },
    });
    return result;
  };
  const record = observer => {
    const result = get(state, observer);
    if (!result) throw new TypeErrorIntrinsic("Illegal invocation");
    return result;
  };
  const install = globalThis.__omoikane_install_security_reporting;
  delete globalThis.__omoikane_install_security_reporting;
  install(({ queueTask }) => {
    function notify(report) {
      for (const observer of registered) add(observer, report);
      buffer.push(report);
      const sameType = buffer.filter(item => item.type === report.type);
      if (sameType.length > 100) buffer.splice(buffer.indexOf(sameType[0]), 1);
    }
    function add(observer, report) {
      const data = record(observer);
      if (data.types.length && !data.types.includes(report.type)) return;
      data.queue.push(copy(report));
      if (data.queue.length !== 1) return;
      const notify = [...registered];
      queueTask(() => {
        for (const target of notify) {
          const current = record(target);
          if (!current.queue.length) continue;
          const reports = current.queue;
          current.queue = [];
          // A throwing callback is reported separately without preventing the
          // remaining observers from receiving their own reports.
          try { apply(current.callback, target, [reports, target]); }
          catch (error) { globalThis.reportError(error); }
        }
      });
    }
    class ReportingObserver {
      constructor(callback, options = {}) {
        if (!arguments.length || typeof callback !== "function") throw new TypeErrorIntrinsic("Expected a callable callback");
        const source = dictionary(options), buffered = !!source.buffered, value = source.types;
        let types = [];
        if (value !== undefined) {
          if (value === null || typeof value !== "object" && typeof value !== "function") throw new TypeErrorIntrinsic("Expected a sequence");
          const iterator = value[Symbol.iterator];
          if (typeof iterator !== "function") throw new TypeErrorIntrinsic("Expected an iterable sequence");
          types = Array.from({ [Symbol.iterator]: () => apply(iterator, value, []) }, string);
        }
        set(state, this, { callback, types, buffered, queue: [] });
      }
      observe() {
        const data = record(this);
        registered.add(this);
        if (!data.buffered) return;
        data.buffered = false;
        for (const report of buffer) queueTask(() => add(this, report));
      }
      disconnect() { record(this); registered.delete(this); }
      takeRecords() {
        const data = record(this), reports = data.queue;
        data.queue = [];
        return reports;
      }
    }
    for (const name of ["observe", "disconnect", "takeRecords"]) {
      Object.defineProperty(ReportingObserver.prototype, name, { enumerable: true });
    }
    Object.defineProperty(ReportingObserver.prototype, Symbol.toStringTag, { value: "ReportingObserver", configurable: true });
    Object.defineProperty(globalThis, "ReportingObserver", { value: ReportingObserver, writable: true, configurable: true });
    return {
      queueCspReport(init) {
        const body = {
          documentURL: init.documentURI, referrer: init.referrer ?? "",
          blockedURL: init.blockedURI || null, effectiveDirective: init.effectiveDirective,
          originalPolicy: init.originalPolicy, sourceFile: init.sourceFile ?? null,
          sample: init.sample ?? "", disposition: init.disposition,
          statusCode: init.statusCode || 0, lineNumber: init.lineNumber ?? null,
          columnNumber: init.columnNumber ?? null,
        };
        const report = { type: "csp-violation", url: init.reportURL, body };
        notify(report);
      },
      queueTestReport(message, url) { notify({ type: "test", url, body: { message } }); },
      queueReport(report) {
        notify({ type: report.type, url: report.url, body: report.body });
      },
    };
  });
})();
