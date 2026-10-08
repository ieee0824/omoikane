// Handler inventory: HTML GlobalEventHandlers / WindowEventHandlers plus
// Pointer/Touch, CSS Animation/Transition/Scroll Snap, Selection and Gamepad.
// Regenerate: python3 scripts/generate-event-handler-inventory.py --wpt-root <WPT>
// The inventory records pinned IDL sources and separately documented additions.
(() => {
  const globalTypes = [
    "abort", "animationcancel", "animationend", "animationiteration", "animationstart", "auxclick",
    "beforeinput", "beforematch", "beforetoggle", "beforexrselect", "blur", "cancel",
    "canplay", "canplaythrough", "change", "click", "close", "command",
    "contentvisibilityautostatechange", "contextlost", "contextmenu", "contextrestored", "copy", "cuechange",
    "cut", "dblclick", "drag", "dragend", "dragenter", "dragleave",
    "dragover", "dragstart", "drop", "durationchange", "emptied", "ended",
    "error", "fencedtreeclick", "focus", "formdata", "gotpointercapture", "input",
    "invalid", "keydown", "keypress", "keyup", "load", "loadeddata",
    "loadedmetadata", "loadstart", "lostpointercapture", "mousedown", "mouseenter", "mouseleave",
    "mousemove", "mouseout", "mouseover", "mouseup", "paste", "pause",
    "play", "playing", "pointercancel", "pointerdown", "pointerenter", "pointerleave",
    "pointermove", "pointerout", "pointerover", "pointerrawupdate", "pointerup", "progress",
    "ratechange", "reset", "resize", "scroll", "scrollend", "scrollsnapchange",
    "scrollsnapchanging", "securitypolicyviolation", "seeked", "seeking", "select", "selectionchange",
    "selectstart", "slotchange", "snapchanged", "snapchanging", "stalled", "submit",
    "suspend", "timeupdate", "toggle", "touchcancel", "touchend", "touchmove",
    "touchstart", "transitioncancel", "transitionend", "transitionrun", "transitionstart", "volumechange",
    "waiting", "webkitanimationend", "webkitanimationiteration", "webkitanimationstart", "webkittransitionend", "wheel",
  ];
  const windowTypes = [
    "afterprint", "beforeprint", "beforeunload", "gamepadconnected", "gamepaddisconnected", "hashchange",
    "languagechange", "message", "messageerror", "offline", "online", "pagehide",
    "pagereveal", "pageshow", "pageswap", "popstate", "portalactivate", "rejectionhandled",
    "storage", "unhandledrejection", "unload",
  ];
  const secureWindowTypes = [
    "devicemotion", "deviceorientation", "deviceorientationabsolute",
  ];
  const get = Function.prototype.call.bind(WeakMap.prototype.get);
  const set = Function.prototype.call.bind(WeakMap.prototype.set);
  const apply = Reflect.apply;
  const states = new WeakMap();
  function targetState(target) {
    let state = get(states, target);
    if (!state) { state = new Map(); set(states, target, state); }
    return state;
  }
  function invokeHandler(target, type, record, event) {
    const callback = record.callback;
    if (!callback) return;
    const specialError = type === "error" && event.type === "error" && target === globalThis && "message" in event;
    const args = specialError ? [event.message, event.filename || "", event.lineno || 0, event.colno || 0, event.error] : [event];
    const result = apply(callback, target, args);
    if (type === "beforeunload" && result != null) {
      event.preventDefault();
      if (event.returnValue === "") event.returnValue = result;
    } else if (specialError ? result === true : result === false) event.preventDefault();
  }
  function descriptor(context, type, global = false) {
    const value = {
      configurable: true, enumerable: true,
      get() {
        const target = context.resolveTarget(global && this == null ? globalThis : this, type);
        return target ? targetState(target).get(type)?.callback || null : null;
      },
      set(value) {
        const target = context.resolveTarget(global && this == null ? globalThis : this, type);
        if (!target) return;
        const state = targetState(target), previous = state.get(type);
        const callback = typeof value === "function" ? value : null;
        if (previous) {
          previous.callback = callback;
          if (!callback) { target.removeEventListener(type, previous.listener); state.delete(type); }
        } else if (callback) {
          const record = { callback, listener: null };
          record.listener = event => invokeHandler(target, type, record, event);
          state.set(type, record);
          target.addEventListener(type, record.listener);
        }
      },
    };
    Object.defineProperty(value.get, "name", { value: "get on" + type, configurable: true });
    Object.defineProperty(value.set, "name", { value: "set on" + type, configurable: true });
    return value;
  }
  const install = globalThis.__omoikane_install_event_handlers;
  delete globalThis.__omoikane_install_event_handlers;
  install(context => {
    const sensorTypes = context.secure ? secureWindowTypes : [];
    context.reflectWindowTypes(windowTypes);
    // Replace legacy slots with the global interface attribute descriptors.
    for (const type of [...globalTypes, ...windowTypes, ...sensorTypes]) {
      delete globalThis["on" + type];
    }
    for (const target of context.globalTargets) {
      for (const type of globalTypes) Object.defineProperty(target, "on" + type, descriptor(context, type, target === globalThis));
    }
    for (const target of context.windowTargets) {
      for (const type of windowTypes) Object.defineProperty(target, "on" + type, descriptor(context, type, target === globalThis));
    }
    for (const type of sensorTypes) Object.defineProperty(context.windowTarget, "on" + type, descriptor(context, type, true));
    return new Set([...globalTypes, ...windowTypes, ...sensorTypes, "compositionstart", "compositionupdate", "compositionend", "fullscreenchange", "fullscreenerror"]);
  });
})();
