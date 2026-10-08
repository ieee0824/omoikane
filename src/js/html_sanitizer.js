// Web IDL conversion and private, cross-Realm state for the Sanitizer API.
// Native filtering receives owned data only, before parsed nodes are connected.
(() => {
  const configure = globalThis.__omoikane_sanitizer_configuration;
  const modify = globalThis.__omoikane_sanitizer_modify;
  delete globalThis.__omoikane_sanitizer_configuration;
  delete globalThis.__omoikane_sanitizer_modify;
  const apply = Reflect.apply;
  const arrayFrom = Array.from;
  const iteratorSymbol = Symbol.iterator;
  const stringify = JSON.stringify;
  const parse = JSON.parse;
  const string = String;
  const TypeErrorIntrinsic = TypeError;
  const create = Object.create;
  const defineProperty = Object.defineProperty;
  const setPrototypeOf = Object.setPrototypeOf;
  const mapGet = Function.prototype.call.bind(WeakMap.prototype.get);
  const mapSet = Function.prototype.call.bind(WeakMap.prototype.set);
  const HTML = 'http://www.w3.org/1999/xhtml';
  function domString(value) {
    if (typeof value === 'symbol') throw new TypeErrorIntrinsic('Cannot convert Symbol to DOMString');
    return string(value);
  }
  function dictionary(value) {
    if (value == null) return create(null);
    if (typeof value !== 'object' && typeof value !== 'function') {
      throw new TypeErrorIntrinsic('Expected dictionary');
    }
    return value;
  }
  function sequence(value, convert) {
    if (value === null || (typeof value !== 'object' && typeof value !== 'function')) {
      throw new TypeErrorIntrinsic('Expected sequence');
    }
    const method = value[iteratorSymbol];
    if (typeof method !== 'function') throw new TypeErrorIntrinsic('Expected iterable sequence');
    const iterable = create(null);
    iterable[iteratorSymbol] = () => apply(method, value, []);
    const result = apply(arrayFrom, undefined, [iterable, convert]);
    setPrototypeOf(result, null);
    return result;
  }
  function name(value, namespace, localAttributes = false) {
    const result = create(null);
    if (value !== null && (typeof value === 'object' || typeof value === 'function')) {
      const inputName = value.name;
      if (inputName === undefined) throw new TypeErrorIntrinsic('Missing required name');
      result.name = domString(inputName);
      const inputNamespace = value.namespace;
      result.namespace = inputNamespace === undefined ? namespace
        : inputNamespace === null ? null : domString(inputNamespace);
      if (result.namespace === '') result.namespace = null;
      if (localAttributes) {
        const attributes = value.attributes;
        if (attributes !== undefined) result.attributes = sequence(attributes, attribute);
        const removed = value.removeAttributes;
        if (removed !== undefined) result.removeAttributes = sequence(removed, attribute);
      }
    } else {
      result.name = domString(value);
      result.namespace = namespace;
    }
    return result;
  }
  function element(value) { return name(value, HTML); }
  function localElement(value) { return name(value, HTML, true); }
  function attribute(value) { return name(value, null); }
  function pi(value) {
    const result = create(null);
    if (value !== null && (typeof value === 'object' || typeof value === 'function')) {
      const target = value.target;
      if (target === undefined) throw new TypeErrorIntrinsic('Missing required target');
      result.target = domString(target);
    } else result.target = domString(value);
    return result;
  }
  const members = [
    ['attributes', attribute], ['comments', null], ['dataAttributes', null],
    ['elements', localElement], ['javascriptURLs', null], ['processingInstructions', pi],
    ['removeAttributes', attribute], ['removeElements', element],
    ['removeProcessingInstructions', pi], ['replaceWithChildrenElements', element],
  ];
  function configuration(input, permissive) {
    if (input != null && typeof input !== 'object' && typeof input !== 'function') {
      if (domString(input) !== 'default') throw new TypeErrorIntrinsic('Unknown sanitizer preset');
      return configure('default', permissive);
    }
    input = dictionary(input);
    const converted = create(null);
    for (let index = 0; index < members.length; index++) {
      const key = members[index][0];
      const conversion = members[index][1];
      const value = input[key];
      if (value !== undefined) converted[key] = conversion ? sequence(value, conversion) : !!value;
    }
    return configure(stringify(converted), permissive);
  }

  globalThis.__omoikane_create_sanitizer_bridge = shared => {
    const states = shared.sanitizerConfigurations || (shared.sanitizerConfigurations = new WeakMap());
    function state(receiver) {
      const value = mapGet(states, receiver);
      if (value === undefined) throw new TypeErrorIntrinsic('Receiver is not a Sanitizer');
      return value;
    }
    function change(receiver, operation, argument) {
      const result = parse(modify(state(receiver), operation, stringify(argument)));
      mapSet(states, receiver, result.configuration);
      return result.changed;
    }
    class Sanitizer {
      constructor(input = 'default') { mapSet(states, this, configuration(input, true)); }
      get() { return parse(parse(modify(state(this), 'get', '{}')).configuration); }
      allowElement(value) {
        state(this); if (!arguments.length) throw new TypeErrorIntrinsic('Element is required');
        return change(this, 'allowElement', localElement(value));
      }
      removeElement(value) {
        state(this); if (!arguments.length) throw new TypeErrorIntrinsic('Element is required');
        return change(this, 'removeElement', element(value));
      }
      replaceElementWithChildren(value) {
        state(this); if (!arguments.length) throw new TypeErrorIntrinsic('Element is required');
        return change(this, 'replaceElementWithChildren', element(value));
      }
      allowAttribute(value) {
        state(this); if (!arguments.length) throw new TypeErrorIntrinsic('Attribute is required');
        return change(this, 'allowAttribute', attribute(value));
      }
      removeAttribute(value) {
        state(this); if (!arguments.length) throw new TypeErrorIntrinsic('Attribute is required');
        return change(this, 'removeAttribute', attribute(value));
      }
      allowProcessingInstruction(value) {
        state(this); if (!arguments.length) throw new TypeErrorIntrinsic('Target is required');
        return change(this, 'allowProcessingInstruction', pi(value));
      }
      removeProcessingInstruction(value) {
        state(this); if (!arguments.length) throw new TypeErrorIntrinsic('Target is required');
        return change(this, 'removeProcessingInstruction', pi(value));
      }
      setComments(value) {
        state(this); if (!arguments.length) throw new TypeErrorIntrinsic('Boolean is required');
        return change(this, 'setComments', !!value);
      }
      setDataAttributes(value) {
        state(this); if (!arguments.length) throw new TypeErrorIntrinsic('Boolean is required');
        return change(this, 'setDataAttributes', !!value);
      }
      setJavascriptURLs(value) {
        state(this); if (!arguments.length) throw new TypeErrorIntrinsic('Boolean is required');
        return change(this, 'setJavascriptURLs', !!value);
      }
      removeUnsafe() { return change(this, 'removeUnsafe', null); }
    }
    for (const key of ['get','allowElement','removeElement','replaceElementWithChildren',
      'allowAttribute','removeAttribute','allowProcessingInstruction','removeProcessingInstruction',
      'setComments','setDataAttributes','setJavascriptURLs','removeUnsafe']) {
      defineProperty(Sanitizer.prototype, key, {enumerable:true});
    }
    defineProperty(Sanitizer.prototype, Symbol.toStringTag, {value:'Sanitizer', configurable:true});
    defineProperty(globalThis, 'Sanitizer', {value:Sanitizer, writable:true, configurable:true});
    function readOptions(options, safe, scriptFlag = true) {
      options = dictionary(options);
      const result = create(null);
      result.runScripts = safe || !scriptFlag ? false : !!options.runScripts;
      const sanitizer = options.sanitizer;
      if (sanitizer === undefined && !safe) result.configuration = null;
      else {
        const branded = sanitizer !== null && (typeof sanitizer === 'object' || typeof sanitizer === 'function')
          ? mapGet(states, sanitizer) : undefined;
        result.configuration = branded === undefined
          ? configuration(sanitizer === undefined ? 'default' : sanitizer, !safe) : branded;
      }
      return result;
    }
    return {readOptions};
  };
})();
