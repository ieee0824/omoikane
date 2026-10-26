# User validity selectors

`:user-valid` and `:user-invalid` share the current constraint-validation result
with `:valid` and `:invalid`, and additionally require the control's user-validity
flag. Only HTML `input`, `textarea`, and `select` elements can acquire this flag.
Forms, fieldsets, buttons, foreign-namespace elements, and form-associated custom
elements do not acquire it. Candidacy changes still apply while the flag is set:
disabling a control suppresses matching, and enabling it reveals its retained
history and current validity.

The DOM element owns the flag, initially false. Adoption preserves it; cloning
and importing nodes initialize it to false. It does not retain a JavaScript
wrapper, form, document, or borrowed reference. A private bootstrap binding
updates it and invalidates styles. Selector queries recursively recognize these
pseudo-classes inside `:is()`, `:where()`, `:not()`, `:has()`, and filtered sibling
selectors so that cached constraint results are refreshed before matching.
State conditions before a generated pseudo-element, such as
`input:user-invalid::before`, apply to the originating control. A condition
after the pseudo-element boundary is evaluated against the pseudo-element.

Submission through activation or `requestSubmit()` marks owned controls before
interactive validation and author `invalid`/`submit` listeners, including when
validation is skipped. `submit()` bypasses that marking step. A successful reset
clears history; cancelling the reset event preserves it. Ownership includes
external controls associated through `form` and the existing parser owner.
Currently disabled controls also acquire the submission flag.

Accepted host text input records editing history privately. Blur commits that
history before `change`; returning to the original value still commits history
without manufacturing a `change` event for an unchanged value. Synthetic events
and script value assignments do not create user interaction history. Reset also
clears pending edits while a control remains focused. Checkable activation marks
before `input`/`change`, after cancellation has been resolved. Select keyboard and
option pointer defaults mark before selection notifications and skip disabled
options and disabled optgroups.

## Temporal input keyboard policy

The component editor keeps incomplete input in private node state. Its DOM value
remains empty until the components form a valid canonical HTML value. Incomplete
nonempty components contribute `validity.badInput`. Script value and numeric
value assignments replace the draft; changing input kind refreshes its shape.

The deterministic field order is month/day/year for dates, month/year for months,
week/year for weeks, and hour/minute/AM-PM for times. Local date-times combine
date and time fields. A seconds field is included when required by the value or
step. Tab and horizontal arrows move between components; Tab leaves the control
at a boundary. Digits replace or append in the current field, Backspace/Delete
clear it, vertical arrows adjust it, and A/P selects the period. Calendar
conversion uses explicit values, without consulting the wall clock or host
locale. Completing a value commits user validity before notifications. A later
partial deletion retains the flag and reflects the invalid draft.

This field order is a host UI policy, not a normative requirement of HTML.
The fixed upstream multifield fixture explicitly describes its keyboard details
as unspecified. With WebDriver Actions key events, Firefox 158.0.1 passes 16 of
the 17 original subtests; its remaining difference is the tentative local
date-time keyboard sequence. An earlier ElementSendKeys comparison produced
additional differences because that command can special-case non-typeable
controls. Preserve both raw reports and distinguish their input transports.
The original fixture assertions remain unchanged.

## Verification

`tests/css_user_valid.rs` covers submission/reset event order, ownership,
copy/adoption/collection, actual host input, cancelled actions, partial temporal
editing, namespace exclusions, nested matching, and warm style invalidation.
Its original submission reftest uses the browser frame API: a paint before the
queued animation callback must differ from the final frame, and that frame must
match the original reference with zero differing pixels. The frame API preserves
the existing page runtime rather than executing the fixture scripts a second
time through the static document renderer.

`tests/wpt/user-valid.json` selects the fixed-revision upstream testharness cases.
`scripts/fetch-wpt.sh` also fetches the original submission reftest and reference.
The WPT driver routes supported keyboard actions through the host input pump;
it does not directly set values or validation flags to satisfy assertions.

```sh
WPT_REQUIRED=1 cargo test --locked --test css_user_valid
WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/user-valid.json \
  cargo test --locked --test wpt_smoke -- --nocapture
```

References: [HTML user-valid selectors](https://html.spec.whatwg.org/multipage/semantics-other.html#selector-user-valid),
[HTML user validity](https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#user-validity),
[Selectors user action validity](https://drafts.csswg.org/selectors-4/#user-pseudos).
