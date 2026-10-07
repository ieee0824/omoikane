// Geometry objects keep coordinates in private state; platform getters and
// JSON serialization therefore cannot be changed by adding public fields.
(() => {
  const rectangleState = new WeakMap();
  const quadState = new WeakMap();
  const pointState = new WeakMap();
  const rectListState = new WeakMap();
  const rectListToken = {};
  const weakGet = Function.prototype.call.bind(WeakMap.prototype.get);
  const weakSet = Function.prototype.call.bind(WeakMap.prototype.set);
  const number = value => +value;
  const min = Math.min, max = Math.max;
  const TypeErrorIntrinsic = TypeError;
  const createObject = Object.create;
  const read = (state, object) => {
    const value = weakGet(state, object);
    if (!value) throw new TypeErrorIntrinsic("Illegal invocation");
    return value;
  };
  const dictionary = value => {
    if (value === null || value === undefined) return {};
    if (typeof value !== "object" && typeof value !== "function") {
      throw new TypeErrorIntrinsic("Expected a dictionary");
    }
    return value;
  };
  function rectValues(value) {
    const init = dictionary(value);
    // WebIDL dictionary members are read in lexicographic order.
    const member = (name, fallback) => {
      const value = init[name];
      return value === undefined ? fallback : number(value);
    };
    const height = member("height", 0);
    const width = member("width", 0);
    const x = member("x", 0);
    const y = member("y", 0);
    return [x, y, width, height];
  }

  function pointValues(value) {
    const init = dictionary(value);
    const member = (name, fallback) => {
      const value = init[name];
      return value === undefined ? fallback : number(value);
    };
    const w = member("w", 1), x = member("x", 0), y = member("y", 0), z = member("z", 0);
    return [x, y, z, w];
  }
  globalThis.__omoikane_geometry_point_values = pointValues;
  function matrixInitRecord(value) {
    const init = dictionary(value), converted = {};
    for (const key of ["a", "b", "c", "d", "e", "f"]) {
      const member = init[key];
      if (member !== undefined) converted[key] = number(member);
    }
    const is2D = init.is2D;
    const values = [];
    const aliases = { m11: "a", m12: "b", m21: "c", m22: "d", m41: "e", m42: "f" };
    for (let column = 1; column <= 4; column++) {
      for (let row = 1; row <= 4; row++) {
        const key = "m" + column + row, alias = aliases[key];
        const raw = init[key], primary = raw === undefined ? undefined : number(raw);
        const alternate = converted[alias];
        if (primary !== undefined && alternate !== undefined &&
            !(primary === alternate || Number.isNaN(primary) && Number.isNaN(alternate))) {
          throw new TypeErrorIntrinsic("Inconsistent matrix aliases");
        }
        const member = primary === undefined ? alternate : primary;
        values.push(member === undefined ? (column === row ? 1 : 0) : member);
      }
    }
    if (is2D !== undefined && Boolean(is2D) && (
        [2, 3, 6, 7, 8, 9, 11, 14].some(index => values[index] !== 0) || values[10] !== 1 || values[15] !== 1)) {
      throw new TypeErrorIntrinsic("Inconsistent two-dimensional matrix");
    }
    const inferred2D = [2, 3, 6, 7, 8, 9, 11, 14].every(index => values[index] === 0) &&
      values[10] === 1 && values[15] === 1;
    return { values, is2D: is2D === undefined ? inferred2D : Boolean(is2D) };
  }
  function matrixValues(value) { return matrixInitRecord(value).values; }
  globalThis.__omoikane_geometry_matrix_init = matrixInitRecord;

  class DOMPointReadOnly {
    constructor(x = 0, y = 0, z = 0, w = 1) {
      weakSet(pointState, this, [number(x), number(y), number(z), number(w)]);
    }
    static fromPoint(other = {}) { return new DOMPointReadOnly(...pointValues(other)); }
    get x() { return read(pointState, this)[0]; }
    get y() { return read(pointState, this)[1]; }
    get z() { return read(pointState, this)[2]; }
    get w() { return read(pointState, this)[3]; }
    matrixTransform(matrix = {}) {
      const point = read(pointState, this);
      const values = matrixValues(matrix);
      const [x, y, z, w] = point;
      return new DOMPoint(
        values[0] * x + values[4] * y + values[8] * z + values[12] * w,
        values[1] * x + values[5] * y + values[9] * z + values[13] * w,
        values[2] * x + values[6] * y + values[10] * z + values[14] * w,
        values[3] * x + values[7] * y + values[11] * z + values[15] * w,
      );
    }
    toJSON() {
      const [x, y, z, w] = read(pointState, this);
      return { x, y, z, w };
    }
  }
  class DOMPoint extends DOMPointReadOnly {
    static fromPoint(other = {}) { return new DOMPoint(...pointValues(other)); }
    get x() { return super.x; }
    set x(value) { read(pointState, this)[0] = number(value); }
    get y() { return super.y; }
    set y(value) { read(pointState, this)[1] = number(value); }
    get z() { return super.z; }
    set z(value) { read(pointState, this)[2] = number(value); }
    get w() { return super.w; }
    set w(value) { read(pointState, this)[3] = number(value); }
  }

  class DOMRectReadOnly {
    constructor(x = 0, y = 0, width = 0, height = 0) {
      weakSet(rectangleState, this, [number(x), number(y), number(width), number(height)]);
    }
    static fromRect(other = {}) { return new DOMRectReadOnly(...rectValues(other)); }
    get x() { return read(rectangleState, this)[0]; }
    get y() { return read(rectangleState, this)[1]; }
    get width() { return read(rectangleState, this)[2]; }
    get height() { return read(rectangleState, this)[3]; }
    get top() {
      const [, y, , height] = read(rectangleState, this);
      return min(y, y + height);
    }
    get right() {
      const [x, , width] = read(rectangleState, this);
      return max(x, x + width);
    }
    get bottom() {
      const [, y, , height] = read(rectangleState, this);
      return max(y, y + height);
    }
    get left() {
      const [x, , width] = read(rectangleState, this);
      return min(x, x + width);
    }
    toJSON() {
      const [x, y, width, height] = read(rectangleState, this);
      return {
        x, y, width, height,
        top: min(y, y + height), right: max(x, x + width),
        bottom: max(y, y + height), left: min(x, x + width),
      };
    }
  }
  class DOMRect extends DOMRectReadOnly {
    static fromRect(other = {}) { return new DOMRect(...rectValues(other)); }
    get x() { return super.x; }
    set x(value) { read(rectangleState, this)[0] = number(value); }
    get y() { return super.y; }
    set y(value) { read(rectangleState, this)[1] = number(value); }
    get width() { return super.width; }
    set width(value) { read(rectangleState, this)[2] = number(value); }
    get height() { return super.height; }
    set height(value) { read(rectangleState, this)[3] = number(value); }
  }
  class DOMQuad {
    constructor(p1 = {}, p2 = {}, p3 = {}, p4 = {}) {
      weakSet(quadState, this, [p1, p2, p3, p4].map(value => new DOMPoint(...pointValues(value))));
    }
    static fromRect(other = {}) {
      const [x, y, width, height] = rectValues(other);
      return new DOMQuad(
        { x, y }, { x: x + width, y },
        { x: x + width, y: y + height }, { x, y: y + height },
      );
    }
    static fromQuad(other = {}) {
      const init = dictionary(other);
      const points = ["p1", "p2", "p3", "p4"].map(name =>
        new DOMPoint(...pointValues(init[name])));
      const result = createObject(DOMQuad.prototype);
      weakSet(quadState, result, points);
      return result;
    }
    get p1() { return read(quadState, this)[0]; }
    get p2() { return read(quadState, this)[1]; }
    get p3() { return read(quadState, this)[2]; }
    get p4() { return read(quadState, this)[3]; }
    getBounds() {
      const points = read(quadState, this).map(point => read(pointState, point));
      const left = min(...points.map(point => point[0]));
      const right = max(...points.map(point => point[0]));
      const top = min(...points.map(point => point[1]));
      const bottom = max(...points.map(point => point[1]));
      return new DOMRect(left, top, right - left, bottom - top);
    }
    toJSON() {
      const [p1, p2, p3, p4] = read(quadState, this);
      const serialize = point => {
        const [x, y, z, w] = read(pointState, point);
        return { x, y, z, w };
      };
      return { p1: serialize(p1), p2: serialize(p2), p3: serialize(p3), p4: serialize(p4) };
    }
  }
  class DOMRectList {
    constructor(token = undefined, rectangles) {
      if (token !== rectListToken) throw new TypeErrorIntrinsic("Illegal constructor");
      weakSet(rectListState, this, rectangles);
    }
    get length() { return read(rectListState, this).length; }
    item(index) {
      if (!arguments.length) throw new TypeErrorIntrinsic("item requires an index");
      return read(rectListState, this)[(+index) >>> 0] ?? null;
    }
    [Symbol.iterator]() { return Array.prototype.values.call(read(rectListState, this)); }
  }
  globalThis.__omoikane_make_rect_list = rectangles => {
    const target = new DOMRectList(rectListToken, rectangles);
    const isIndex = key => typeof key === "string" && /^(?:0|[1-9]\d*)$/.test(key) && Number(key) < 4294967295;
    const proxy = new Proxy(target, {
      get(_target, key, receiver) { return isIndex(key) ? rectangles[+key] : Reflect.get(target, key, receiver); },
      has(_target, key) { return isIndex(key) ? +key < rectangles.length : key in target; },
      ownKeys() { return rectangles.map((_, index) => String(index)).concat(Reflect.ownKeys(target)); },
      getOwnPropertyDescriptor(_target, key) {
        if (isIndex(key) && +key < rectangles.length) {
          return { value: rectangles[+key], writable: false, enumerable: true, configurable: true };
        }
        return Reflect.getOwnPropertyDescriptor(target, key);
      },
      set(_target, key, value, receiver) { return !isIndex(key) && Reflect.set(target, key, value, receiver); },
      defineProperty(_target, key, descriptor) { return !isIndex(key) && Reflect.defineProperty(target, key, descriptor); },
      deleteProperty(_target, key) { return !(isIndex(key) && +key < rectangles.length) && Reflect.deleteProperty(target, key); },
      preventExtensions() { return false; },
    });
    weakSet(rectListState, proxy, rectangles);
    return proxy;
  };
  for (const type of [DOMPointReadOnly, DOMPoint, DOMRectReadOnly, DOMRect, DOMQuad, DOMRectList]) {
    Object.defineProperty(type.prototype, Symbol.toStringTag, { value: type.name, configurable: true });
    // WebIDL prototype operations and attributes are enumerable.
    for (const name of Object.getOwnPropertyNames(type.prototype)) {
      if (name !== "constructor") Object.defineProperty(type.prototype, name, { enumerable: true });
    }
    for (const name of Object.getOwnPropertyNames(type)) {
      if (typeof type[name] === "function") Object.defineProperty(type, name, { enumerable: true });
    }
    Object.defineProperty(globalThis, type.name, { value: type, writable: true, configurable: true });
  }
})();
