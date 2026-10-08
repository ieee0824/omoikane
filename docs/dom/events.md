# Platform events and handler attributes

Issue #1290 adds PopStateEvent, HashChangeEvent, PageTransitionEvent,
BeforeUnloadEvent, SubmitEvent, FormDataEvent, Touch,
ContentVisibilityAutoStateChangeEvent, DeviceOrientationEvent and DeviceMotionEvent.
Event payloads use private branded records and Web IDL dictionary conversion.
Interface-valued dictionary members consult private records in the object's
creation Realm. Genuine EventTarget and HTMLElement values remain valid across
Realms and prototype changes; prototype-only impostors are rejected. Registered
WindowProxy values retain their EventTarget identity.
The native touch input adapter assigns an omitted contact identifier from its
position while preserving supplied identifiers; the public Touch constructor
still requires identifier and target.
BeforeUnloadEvent and the nested motion payload interfaces cannot be constructed
directly. TouchEvent contains snapshot TouchList objects with readonly indices.
The existing navigation, lifecycle, form and native touch dispatch paths use the
captured platform constructors, including when page code replaces public names.
Iframe history and fragment events are created and dispatched in the active
child Realm. Its private factory is traced as Realm metadata, so it follows the
Realm's lifetime and does not depend on page-visible constructor properties.
Removal snapshots frame IDs in native tree order before dispatching departure
events. Unrelated descendants do not require temporary JS node lists or views;
both loaded frames and unused WindowProxy objects are still retired.

Motion interfaces and their Window handlers require a secure context. Permission
requests consult the runtime permission provider; absent a sensor provider, the
result is denied. No sensor measurements are manufactured.

## Handler inventory

`src/js/event_handler_inventory.json` records the pinned WPT revision, source
hashes and all GlobalEventHandlers/WindowEventHandlers declarations, including
partial mixins. `scripts/event-handler-extras.json` separately records additions
and their specification sources. Regenerate or check with:

```sh
WPT_ROOT=target/wpt scripts/fetch-wpt.sh
python3 scripts/generate-event-handler-inventory.py --wpt-root target/wpt
python3 scripts/generate-event-handler-inventory.py --wpt-root target/wpt --check
```

Generation requires the complete, unmodified pinned `interfaces/` tree. The
inventory currently includes 108 global, 21 Window and three secure Window
handler names. CI checks reproducibility. The integration test dispatches every
inventory entry and verifies assignment, removal and body/frameset reflection.
Replacing an attribute callback preserves its existing event listener position.
Parser-authored inline handlers are installed before author scripts. Repeating
initialization preserves later IDL assignments; an explicit `setAttribute` call
reinstalls the attribute handler even when its source text is unchanged.

Window handlers are own accessor properties of the global object. Element and
Document handlers are inherited from their interface prototypes. A global
accessor accepts an undefined/null receiver as the global object, while unrelated
receivers fail branding. Worker globals retain their shared handlers and remove
Window-only interfaces and attributes.

## Legacy current event

`window.event` uses an owned, GC-traced per-document slot. Dispatch selects the
listener's associated Realm before looking up `handleEvent`, saves the previous
event and restores it after normal or abrupt completion. Shadow-tree listeners
do not replace the slot. Ordinary, bound and proxy callable listeners and object
listeners are covered by cross-Realm integration tests. Object creation Realm
metadata is independent of subsequent prototype changes. Builtin intrinsic
objects and their prototypes also record their creation Realm.

Native resize and viewport scroll notifications invoke handler attributes through
the normal event dispatch path, preserving listener order and invoking each
handler once. Event handler attributes on XHR, upload, AbortSignal, FileReader
and the other existing event targets also set and restore the current event slot.

The getter resolves its receiver's Window rather than its own execution Realm.
A private WeakMap records iframe/auxiliary proxy identities without retaining
keys. Its captured builtin methods avoid author-modified prototype methods.
Iframe reads refresh the current Document and apply the existing origin and
sandbox checks. The integration tests cover borrowed accessors, invalid
receivers, proxy navigation, forced collection and WeakMap method overrides.
Auxiliary WindowProxy property definitions, descriptors and deletions forward
to the active Window, so replacing `event` through a borrowed setter remains
visible through the proxy.

The Boa changes also repair dynamic Function scope indices for nested named
classes. Dedicated engine tests retain named class self-reference and closed-over
lexical variables. Sticky RegExp matching preserves original input context while
matching only at lastIndex; named capture index arrays retain object identity.
See `engine/README.md` for regress provenance and engine validation commands.

## Pinned WPT results and specification conflict

Revision: `dc97e7bed3096ac9e0e591ab5fa22e7fb8844ead`.
The selected unmodified tests produced these results on 2026-10-07:

| Test | Passing subtests | Result |
| --- | ---: | --- |
| SubmitEvent.window.js | 6/6 | PASS |
| FormDataEvent.window.js | 2/2 | PASS |
| Body-FrameSet-Event-Handlers.html | 48/48 | PASS |
| event-global-set-before-handleEvent-lookup.window.js | 1/1 | PASS |
| window-event-restored-after-throwing-onerror.html | 1/1 | PASS |
| event-handler-attribute-replace-preserves-passive.html | 2/2 | PASS |
| Event-dispatch-handlers-changed.html | 1/1 | PASS |
| touch-touchevent-constructor.html | 5/5 | PASS |
| touch-events/idlharness.window.js | 124/128 | FAIL |
| html/dom/idlharness.https.html, six navigation/form interfaces | 53/53 | PASS |

Touch IDL has four contradictory assertions for Window's ontouchstart,
ontouchend, ontouchmove and ontouchcancel. The GlobalEventHandlers mixin tests
require inherited attributes (`resources/idlharness.js`, object member tests),
while the Window interface tests require own attributes and their absence from
Window.prototype. Both requirements cannot hold for the same property.
[Web IDL attributes](https://webidl.spec.whatwg.org/#es-attributes) requires own
attributes for interfaces marked Global. Omoikane follows this rule. All other
Touch IDL subtests pass within the unchanged normal harness deadline.

The manifest records only the four exact mixin failures, with an expiry; new
failures or unexpected passes fail verification. The global descriptor,
undefined-receiver and EventTarget inheritance semantics are independently
asserted in `tests/event_interfaces.rs`. A Firefox headless reference run with
`dom.w3c_touch_events.enabled=1` also fails the same four Window mixin assertions;
it additionally lacks other Touch members, so it is not treated as a complete
passing reference. Original WPT files and assertions are unchanged.
