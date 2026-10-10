// Testdriver requests are executed by the host runner between page jobs.
globalThis.__wpt_window_commands = [];
test_driver_internal.minimize_window = () => new Promise(resolve => {
  __wpt_window_commands.push({hidden: true, resolve});
});
test_driver_internal.set_window_rect = () => new Promise(resolve => {
  __wpt_window_commands.push({hidden: false, resolve});
});
globalThis.__wpt_action_commands = [];
const wptMouseSources = new Map();
test_driver_internal.action_sequence = (sources, context = null) => new Promise((resolve, reject) => {
  if (context !== null && context !== globalThis) {
    reject(new Error('Actions in another browsing context are unsupported'));
    return;
  }
  __wpt_action_commands.push({sources, resolve, reject, tick: 0});
});

function wptActionDuration(action) {
  const duration = action.duration ?? 0;
  if (!Number.isSafeInteger(duration) || duration < 0) {
    throw new Error('Invalid action duration');
  }
  return duration;
}

function wptMousePosition(action, source) {
  if (!Number.isSafeInteger(action.x) || !Number.isSafeInteger(action.y)) {
    throw new Error('Invalid pointer coordinates');
  }
  let x = 0, y = 0;
  if (action.origin === 'pointer') {
    x = source.x; y = source.y;
  } else if (action.origin !== undefined && action.origin !== 'viewport') {
    if (!(action.origin instanceof Element) || !action.origin.isConnected) {
      throw new Error('Stale or invalid element origin');
    }
    const rect = action.origin.getClientRects()[0];
    if (!rect) throw new Error('Element origin has no rendered rectangle');
    const left = Math.max(0, Math.min(rect.x, rect.x + rect.width));
    const right = Math.min(innerWidth, Math.max(rect.x, rect.x + rect.width));
    const top = Math.max(0, Math.min(rect.y, rect.y + rect.height));
    const bottom = Math.min(innerHeight, Math.max(rect.y, rect.y + rect.height));
    if (left > right || top > bottom) throw new Error('Element origin is outside the viewport');
    x = Math.floor((left + right) / 2); y = Math.floor((top + bottom) / 2);
  }
  x += action.x; y += action.y;
  if (x < 0 || y < 0 || x > innerWidth || y > innerHeight) {
    throw new Error('Pointer move is outside the viewport');
  }
  return {x, y};
}

function wptDispatchMouse(source, type, button = 0, previous = source) {
  const target = document.elementFromPoint(source.x, source.y) || document;
  __omoikane_dispatch_mouse_input(target.__id, type, {
    clientX: source.x, clientY: source.y,
    pageX: source.x + scrollX, pageY: source.y + scrollY,
    screenX: source.x, screenY: source.y,
    movementX: source.x - previous.x, movementY: source.y - previous.y,
    buttons: source.buttons, button, pointerId: source.pointerId,
  }, type === 'mousedown');
  return target;
}

function wptPerformMouseAction(action, source) {
  if (action.type === 'pointerMove') {
    // Interpolated timed moves require an implementation in the input pump.
    // Reject them explicitly instead of silently replacing them with a jump.
    if (wptActionDuration(action) !== 0) throw new Error('Timed pointer moves are unsupported');
    const previous = {x: source.x, y: source.y};
    Object.assign(source, wptMousePosition(action, source));
    wptDispatchMouse(source, 'mousemove', 0, previous);
    return;
  }
  if (action.type !== 'pointerDown' && action.type !== 'pointerUp') {
    throw new Error('Unsupported mouse action: ' + action.type);
  }
  const button = action.button;
  if (!Number.isInteger(button) || button < 0 || button > 4) throw new Error('Invalid mouse button');
  const mask = button === 0 ? 1 : button === 1 ? 4 : button === 2 ? 2 : 1 << button;
  if (action.type === 'pointerDown') {
    if (source.buttons & mask) return;
    source.buttons |= mask;
    source.pressedTargets.set(button, wptDispatchMouse(source, 'mousedown', button));
  } else {
    if (!(source.buttons & mask)) return;
    source.buttons &= ~mask;
    const target = wptDispatchMouse(source, 'mouseup', button);
    if (button === 0 && source.pressedTargets.get(button) === target) {
      wptDispatchMouse(source, 'click', button);
    }
    source.pressedTargets.delete(button);
  }
}

function wptPrepareActions(command) {
  if (!Array.isArray(command.sources)) throw new Error('Invalid action sources');
  const ids = new Set();
  for (const input of command.sources) {
    if (typeof input.id !== 'string' || ids.has(input.id) || !Array.isArray(input.actions)) {
      throw new Error('Invalid or duplicate action source');
    }
    ids.add(input.id);
    if (input.type !== 'none' &&
        (input.type !== 'pointer' || (input.parameters?.pointerType ?? 'mouse') !== 'mouse')) {
      throw new Error('Unsupported action source: ' + input.type);
    }
    if (input.type === 'pointer' && !wptMouseSources.has(input.id)) {
      wptMouseSources.set(input.id, {
        x: 0, y: 0, buttons: 0, pointerId: wptMouseSources.size + 1, pressedTargets: new Map(),
      });
    }
  }
  command.prepared = true;
}

globalThis.__wpt_advance_action_tick = function() {
  const command = __wpt_action_commands[0];
  if (!command) return 0;
  try {
    if (!command.prepared) wptPrepareActions(command);
    let duration = 0;
    for (const input of command.sources) {
      const action = input.actions[command.tick];
      if (!action) continue;
      duration = Math.max(duration, wptActionDuration(action));
      if (action.type === 'pause') continue;
      if (input.type === 'none') throw new Error('A none source only accepts pause');
      wptPerformMouseAction(action, wptMouseSources.get(input.id));
    }
    command.tick++;
    if (command.sources.every(input => command.tick >= input.actions.length)) {
      __wpt_action_commands.shift();
      command.resolve();
    }
    return duration;
  } catch (error) {
    __wpt_action_commands.shift();
    command.reject(error);
    return 0;
  }
};
