// requestIdleCallback and Prioritized Task Scheduling (Issue #1293).
//
// The host starts idle periods (see idle_period.rs) through this exported
// hook: `start(budgetMs)` begins a period and `step()` runs one callback.
export const __omoikane_idle_period = {};
(() => {
  "use strict";

  const nativeRequestIdlePeriod = globalThis.__omoikane_request_idle_period;
  const nativeIdleBudget = globalThis.__omoikane_idle_budget;
  delete globalThis.__omoikane_request_idle_period;
  delete globalThis.__omoikane_idle_budget;
  if (typeof nativeRequestIdlePeriod !== "function" || typeof nativeIdleBudget !== "function") {
    return;
  }

  const hostSetTimeout = globalThis.setTimeout;
  const hostClearTimeout = globalThis.clearTimeout;
  const hostPerformance = globalThis.performance;
  const performanceNow = hostPerformance.now;
  const now = () => performanceNow.call(hostPerformance);
  const reportException = globalThis.reportError;
  const callEventListener = globalThis.__omoikane_call_event_listener;
  const PromiseIntrinsic = Promise;
  const queueMicrotaskIntrinsic = globalThis.queueMicrotask;
  const OriginalEvent = globalThis.Event;
  const OriginalAbortSignal = globalThis.AbortSignal;
  const OriginalAbortController = globalThis.AbortController;
  const OriginalDOMException = globalThis.DOMException;
  const constructionToken = {};

  function defineMethods(prototype, methods) {
    for (const [name, value] of Object.entries(methods)) {
      Object.defineProperty(prototype, name, {
        configurable: true, enumerable: true, writable: true, value,
      });
    }
  }
  function defineGetters(prototype, getters) {
    for (const [name, get] of Object.entries(getters)) {
      Object.defineProperty(get, "name", { value: "get " + name });
      Object.defineProperty(prototype, name, { configurable: true, enumerable: true, get });
    }
  }
  function defineToStringTag(prototype, name) {
    Object.defineProperty(prototype, Symbol.toStringTag, { configurable: true, value: name });
  }
  function defineGlobal(name, value) {
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
  }

  // The scheduling state scheduler.yield() inherits: that of the running
  // postTask task, continuation or idle callback. Promise reactions queued
  // while it runs still see it; it resets after them, before the next task.
  // Propagation through later promise jobs is not tracked.
  let currentState = null;
  function enterSchedulingState(state) {
    currentState = state;
  }
  function leaveSchedulingState() {
    queueMicrotaskIntrinsic(() => { currentState = null; });
  }

  // ---- requestIdleCallback ------------------------------------------------

  // Idle request callbacks wait for the next idle period; runnable ones were
  // moved at the start of the current period. Both keep posting order. Each
  // entry is { callback, timeoutAt } with timeoutAt on the performance clock.
  const idleRequests = new Map();
  const runnableIdle = new Map();
  let idleHandle = 0;
  let periodStart = 0;
  const idleDeadlineState = new WeakMap();
  // The page clock stands still while script runs, so the budget measured
  // now still counts from the start of the period.
  const periodDeadline = () => periodStart + nativeIdleBudget();

  class IdleDeadline {
    constructor() {
      const [token, deadline, didTimeout] = arguments;
      if (token !== constructionToken) throw new TypeError("Illegal constructor");
      idleDeadlineState.set(this, { deadline, didTimeout });
    }
  }
  function deadlineState(value) {
    const state = idleDeadlineState.get(value);
    if (!state) throw new TypeError("Illegal invocation");
    return state;
  }
  defineMethods(IdleDeadline.prototype, {
    timeRemaining: function timeRemaining() {
      return Math.max(0, deadlineState(this).deadline() - now());
    },
  });
  defineGetters(IdleDeadline.prototype, {
    didTimeout: function () { return deadlineState(this).didTimeout; },
  });
  defineToStringTag(IdleDeadline.prototype, "IdleDeadline");

  // Idle callbacks run at background priority: scheduler.yield() inside one
  // continues at that priority.
  const idleSchedulingState = Object.freeze({ prioritySource: "background", abortSource: null });
  function invokeIdleCallback(callback, deadline, didTimeout) {
    enterSchedulingState(idleSchedulingState);
    try {
      callback.call(undefined, new IdleDeadline(constructionToken, deadline, didTimeout));
    } catch (error) {
      reportException(error);
    } finally {
      leaveSchedulingState();
    }
  }
  function invokeTimedOut(callback) {
    const deadline = now();
    invokeIdleCallback(callback, () => deadline, true);
  }
  function checkWindowReceiver(receiver) {
    if (receiver !== undefined && receiver !== null && receiver !== globalThis &&
        receiver !== globalThis.window) {
      throw new TypeError("Illegal invocation");
    }
  }

  function requestIdleCallback(callback, options = undefined) {
    checkWindowReceiver(this);
    if (arguments.length < 1) {
      throw new TypeError("requestIdleCallback requires 1 argument");
    }
    if (typeof callback !== "function") {
      throw new TypeError("requestIdleCallback callback must be a function");
    }
    if (options !== undefined && options !== null && typeof options !== "object") {
      throw new TypeError("IdleRequestOptions must be an object");
    }
    const timeout = options == null || options.timeout === undefined
      ? 0 : Number(options.timeout) >>> 0;
    const handle = ++idleHandle;
    idleRequests.set(handle, { callback, timeoutAt: timeout > 0 ? now() + timeout : Infinity });
    if (timeout > 0) {
      hostSetTimeout(() => {
        const entry = idleRequests.get(handle) || runnableIdle.get(handle);
        if (!entry) return;
        idleRequests.delete(handle);
        runnableIdle.delete(handle);
        invokeTimedOut(entry.callback);
      }, timeout);
    }
    nativeRequestIdlePeriod();
    return handle;
  }
  function cancelIdleCallback(handle) {
    checkWindowReceiver(this);
    if (arguments.length < 1) {
      throw new TypeError("cancelIdleCallback requires 1 argument");
    }
    const id = Number(handle) >>> 0;
    idleRequests.delete(id);
    runnableIdle.delete(id);
  }

  __omoikane_idle_period.start = () => {
    periodStart = now();
    for (const [handle, entry] of idleRequests) runnableIdle.set(handle, entry);
    idleRequests.clear();
    if (runnableIdle.size > 0 && periodDeadline() > now()) return true;
    if (runnableIdle.size > 0) nativeRequestIdlePeriod();
    return false;
  };
  __omoikane_idle_period.step = () => {
    if (runnableIdle.size === 0) return false;
    const [handle, entry] = runnableIdle.entries().next().value;
    if (entry.timeoutAt <= now()) {
      // The page clock lags real time while script keeps the event loop
      // busy; a callback whose timeout passed in real time has timed out.
      runnableIdle.delete(handle);
      invokeTimedOut(entry.callback);
    } else if (now() >= periodDeadline()) {
      // The period ended; the rest run in the next one.
      nativeRequestIdlePeriod();
      return false;
    } else {
      runnableIdle.delete(handle);
      invokeIdleCallback(entry.callback, periodDeadline, false);
    }
    if (runnableIdle.size > 0 && now() >= periodDeadline()) nativeRequestIdlePeriod();
    return runnableIdle.size > 0 && now() < periodDeadline();
  };

  if (globalThis.document && typeof globalThis.document === "object") {
    defineGlobal("IdleDeadline", IdleDeadline);
    for (const [name, value] of [["requestIdleCallback", requestIdleCallback],
      ["cancelIdleCallback", cancelIdleCallback]]) {
      Object.defineProperty(globalThis, name, {
        configurable: true, enumerable: true, writable: true, value,
      });
    }
  }

  // ---- Prioritized Task Scheduling ---------------------------------------

  const priorities = ["user-blocking", "user-visible", "background"];
  // AbortSignal brand check that also accepts signals from other Realms,
  // such as an iframe's TaskController.
  function isAbortSignal(value) {
    if (value instanceof OriginalAbortSignal) return true;
    return value !== null && typeof value === "object" && typeof value.aborted === "boolean" &&
      typeof value.addEventListener === "function" && "reason" in value;
  }
  function taskPriority(value) {
    const priority = String(value);
    if (!priorities.includes(priority)) {
      throw new TypeError(`The provided value '${priority}' is not a valid TaskPriority`);
    }
    return priority;
  }

  const signalPriority = new WeakMap();
  const signalPriorityChanging = new WeakSet();
  const dependentSignals = new WeakMap();
  const prioritySourceOf = new WeakMap();

  class TaskSignal extends OriginalAbortSignal {
    constructor(token, priority) {
      if (token !== constructionToken) throw new TypeError("Illegal constructor");
      super();
      signalPriority.set(this, priority);
      this.onprioritychange = null;
    }
  }
  function signalPriorityOf(signal) {
    if (!signalPriority.has(signal)) throw new TypeError("Illegal invocation");
    return signalPriority.get(signal);
  }
  defineGetters(TaskSignal.prototype, {
    priority: function () { return signalPriorityOf(this); },
  });
  Object.defineProperty(TaskSignal, "any", {
    configurable: true, enumerable: true, writable: true,
    value: function any(signals, init = undefined) {
      const sources = Array.from(signals, signal => {
        if (!isAbortSignal(signal)) {
          throw new TypeError("TaskSignal.any requires AbortSignal values");
        }
        return signal;
      });
      const source = init == null ? undefined : init.priority;
      // A composite priority source is replaced by the signal it follows, so
      // dependents of one controller change in creation order.
      const prioritySource = signalPriority.has(source)
        ? prioritySourceOf.get(source) || source : null;
      const priority = prioritySource ? signalPriorityOf(prioritySource)
        : source === undefined ? "user-visible" : taskPriority(source);
      const result = new TaskSignal(constructionToken, priority);
      followAbort(result, sources);
      if (prioritySource) {
        const list = dependentSignals.get(prioritySource) || [];
        list.push(new WeakRef(result));
        dependentSignals.set(prioritySource, list);
        prioritySourceOf.set(result, prioritySource);
      }
      return result;
    },
  });
  defineToStringTag(TaskSignal.prototype, "TaskSignal");

  function abortSignal(signal, reason) {
    if (signal.aborted) return;
    signal.aborted = true;
    signal.reason = reason;
    const event = new OriginalEvent("abort");
    signal.dispatchEvent(event);
    if (typeof signal.onabort === "function") callEventListener(signal.onabort, signal, event);
  }
  function followAbort(result, sources) {
    for (const source of sources) {
      if (source.aborted) {
        abortSignal(result, source.reason);
        return;
      }
    }
    for (const source of sources) {
      source.addEventListener("abort", () => abortSignal(result, source.reason), { once: true });
    }
  }

  class TaskPriorityChangeEvent extends OriginalEvent {
    constructor(type, eventInitDict) {
      if (arguments.length < 2) {
        throw new TypeError("TaskPriorityChangeEvent requires 2 arguments");
      }
      if (eventInitDict == null || typeof eventInitDict !== "object" ||
          eventInitDict.previousPriority === undefined) {
        throw new TypeError("TaskPriorityChangeEventInit requires previousPriority");
      }
      const previousPriority = taskPriority(eventInitDict.previousPriority);
      super(type, eventInitDict);
      priorityChangePrevious.set(this, previousPriority);
    }
  }
  const priorityChangePrevious = new WeakMap();
  defineGetters(TaskPriorityChangeEvent.prototype, {
    previousPriority: function () {
      if (!priorityChangePrevious.has(this)) throw new TypeError("Illegal invocation");
      return priorityChangePrevious.get(this);
    },
  });
  defineToStringTag(TaskPriorityChangeEvent.prototype, "TaskPriorityChangeEvent");

  // TaskSignal "signal priority change".
  function changeSignalPriority(signal, priority) {
    if (signalPriorityChanging.has(signal)) {
      throw new OriginalDOMException("The priority is already changing.", "NotAllowedError");
    }
    const previousPriority = signalPriority.get(signal);
    if (previousPriority === priority) return;
    signalPriorityChanging.add(signal);
    try {
      signalPriority.set(signal, priority);
      const event = new TaskPriorityChangeEvent("prioritychange", { previousPriority });
      signal.dispatchEvent(event);
      if (typeof signal.onprioritychange === "function") {
        callEventListener(signal.onprioritychange, signal, event);
      }
      for (const ref of dependentSignals.get(signal) || []) {
        const dependent = ref.deref();
        if (dependent) changeSignalPriority(dependent, priority);
      }
    } finally {
      signalPriorityChanging.delete(signal);
    }
  }

  class TaskController extends OriginalAbortController {
    constructor(init = undefined) {
      const priority = init == null || init.priority === undefined
        ? "user-visible" : taskPriority(init.priority);
      super();
      this.signal = new TaskSignal(constructionToken, priority);
    }
  }
  defineMethods(TaskController.prototype, {
    setPriority: function setPriority(priority) {
      if (arguments.length < 1) throw new TypeError("setPriority requires 1 argument");
      if (!(this.signal instanceof TaskSignal)) throw new TypeError("Illegal invocation");
      changeSignalPriority(this.signal, taskPriority(priority));
    },
  });
  defineToStringTag(TaskController.prototype, "TaskController");

  // Scheduled tasks. Each holds its priority source (a fixed priority or a
  // TaskSignal) and a sequence number giving its place in its queue. A
  // priority change keeps that place: queues are ordered by enqueue order. One
  // host task runs per scheduled task and always takes the most urgent one.
  const scheduledTasks = new Set();
  let taskSequence = 0;

  function effectivePriority(task) {
    return typeof task.prioritySource === "string"
      ? task.prioritySource : signalPriority.get(task.prioritySource);
  }
  function rank(task) {
    return priorities.indexOf(effectivePriority(task)) * 2 + (task.continuation ? 0 : 1);
  }
  function runNextScheduledTask() {
    let next = null;
    for (const task of scheduledTasks) {
      if (!task.ready) continue;
      if (!next || rank(task) < rank(next) ||
          (rank(task) === rank(next) && task.sequence < next.sequence)) {
        next = task;
      }
    }
    if (!next) return;
    scheduledTasks.delete(next);
    enterSchedulingState(next.state);
    // Aborting the signal while the callback runs still rejects the result.
    next.running = true;
    try {
      next.run();
    } finally {
      next.running = false;
      next.detachAbort();
      leaveSchedulingState();
    }
  }
  function schedule(task, delay) {
    task.sequence = ++taskSequence;
    scheduledTasks.add(task);
    if (task.signal) {
      const onAbort = () => {
        if (task.running) {
          task.reject(task.signal.reason);
          return;
        }
        if (!scheduledTasks.delete(task)) return;
        if (task.timer !== null) hostClearTimeout(task.timer);
        task.reject(task.signal.reason);
      };
      task.signal.addEventListener("abort", onAbort, { once: true });
      task.detachAbort = () => task.signal.removeEventListener("abort", onAbort);
    } else {
      task.detachAbort = () => {};
    }
    // A task without delay is queued now; a delayed one when its delay ends.
    // Either way its host task runs the most urgent queued task.
    task.ready = delay === 0;
    task.timer = hostSetTimeout(() => {
      task.timer = null;
      if (!task.ready) {
        task.ready = true;
        task.sequence = ++taskSequence;
      }
      runNextScheduledTask();
    }, delay);
  }
  function newTask(state, continuation, run, resolve, reject) {
    return {
      prioritySource: state.prioritySource, signal: state.abortSource, state,
      continuation, run, resolve, reject, ready: false, running: false, sequence: 0,
      timer: null,
    };
  }

  function toDelay(value) {
    const number = Number(value);
    if (!Number.isFinite(number)) return 0;
    return Math.max(0, Math.trunc(number));
  }
  function postTask(callback, options) {
    if (typeof callback !== "function") {
      throw new TypeError("postTask callback must be a function");
    }
    if (options != null && typeof options !== "object") {
      throw new TypeError("SchedulerPostTaskOptions must be an object");
    }
    // Dictionary members convert in lexicographic order.
    const delay = options == null || options.delay === undefined ? 0 : toDelay(options.delay);
    const priority = options == null || options.priority === undefined
      ? undefined : taskPriority(options.priority);
    const signal = options == null || options.signal === undefined ? undefined : options.signal;
    if (signal !== undefined && !isAbortSignal(signal)) {
      throw new TypeError("signal must be an AbortSignal");
    }
    let resolve, reject;
    const promise = new PromiseIntrinsic((res, rej) => { resolve = res; reject = rej; });
    if (signal && signal.aborted) {
      reject(signal.reason);
      return promise;
    }
    const prioritySource = priority !== undefined ? priority
      : signalPriority.has(signal) ? signal : "user-visible";
    const state = { prioritySource, abortSource: signal || null };
    const run = () => {
      try {
        resolve(callback.call(undefined));
      } catch (error) {
        reject(error);
      }
    };
    schedule(newTask(state, false, run, resolve, reject), delay);
    return promise;
  }

  const schedulerBrand = new WeakSet();
  class Scheduler {
    constructor(token) {
      if (token !== constructionToken) throw new TypeError("Illegal constructor");
      schedulerBrand.add(this);
    }
  }
  defineMethods(Scheduler.prototype, {
    postTask: function postTask_(callback, options = undefined) {
      try {
        if (!schedulerBrand.has(this)) throw new TypeError("Illegal invocation");
        if (arguments.length < 1) throw new TypeError("postTask requires 1 argument");
        return postTask(callback, options);
      } catch (error) {
        return PromiseIntrinsic.reject(error);
      }
    },
    yield: function yield_() {
      if (!schedulerBrand.has(this)) {
        return PromiseIntrinsic.reject(new TypeError("Illegal invocation"));
      }
      const state = currentState || { prioritySource: "user-visible", abortSource: null };
      let resolve, reject;
      const promise = new PromiseIntrinsic((res, rej) => { resolve = res; reject = rej; });
      if (state.abortSource && state.abortSource.aborted) {
        reject(state.abortSource.reason);
        return promise;
      }
      schedule(newTask(state, true, () => resolve(undefined), resolve, reject), 0);
      return promise;
    },
  });
  Object.defineProperty(Scheduler.prototype.postTask, "name", { value: "postTask" });
  Object.defineProperty(Scheduler.prototype.yield, "name", { value: "yield" });
  defineToStringTag(Scheduler.prototype, "Scheduler");

  const scheduler = new Scheduler(constructionToken);
  const schedulerGetter = function () { return scheduler; };
  Object.defineProperty(schedulerGetter, "name", { value: "get scheduler" });
  // [Replaceable]: assignment shadows the attribute with a data property.
  const schedulerSetter = function (value) {
    Object.defineProperty(globalThis, "scheduler", {
      configurable: true, enumerable: true, writable: true, value,
    });
  };
  Object.defineProperty(schedulerSetter, "name", { value: "set scheduler" });
  Object.defineProperty(globalThis, "scheduler", {
    configurable: true, enumerable: true, get: schedulerGetter, set: schedulerSetter,
  });
  defineGlobal("Scheduler", Scheduler);
  defineGlobal("TaskController", TaskController);
  defineGlobal("TaskSignal", TaskSignal);
  defineGlobal("TaskPriorityChangeEvent", TaskPriorityChangeEvent);
})();
