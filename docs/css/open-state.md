# `:open` state

Omoikane supports the Selectors Level 4 `:open` pseudo-class for HTML
`dialog`, `details`, and dropdown `select` elements. An arbitrary `open`
attribute and an open popover do not establish this state. Picker-capable
`input` elements will use the same native state when their picker surfaces are
connected under #1297.

`dialog` and `details` derive the state from their semantic `open` attribute,
including while CSS does not render the element. Dialog `show()`,
`showModal()`, `close()`, `requestClose()`, and direct `open` changes invalidate
dependent styles. Opening a named `details` element synchronously closes an
open peer with the same non-empty name in the same tree. Activating its first
`summary` toggles that state.

A dropdown `select` matches while its native host picker is presented. Pointer
activation and `showPicker()` open it; Escape cancels it; Enter commits the
highlighted option after closing it. Input and change listeners therefore see
the closed state. A listbox (`multiple` or `size` greater than one) does not
match. Picker state is owned by the native DOM element, invalidates style
caches, closes on removal, and is not exposed through an author-writable
property.

`showPicker()` rejects disabled selects and requires transient user activation.
The broader picker surfaces and host dialog integration are tracked in #1297.
