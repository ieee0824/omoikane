# Autofill selector state

Omoikane matches `:autofill` and its required `:-webkit-autofill` alias only
for HTML `input` elements filled through `JsRuntime::autofill_input`. The
browser-owned bit is stored with the native DOM element so selector queries,
stylesheet matching, `:has()`, filtered siblings, and generated styles observe
one state without calling author JavaScript.

The host operation enters the input's owning Realm and updates its live value
through the captured private input dispatcher. The primitive is removed before
author scripts run. A successful user edit clears the bit and invalidates
dependent styles; a canceled `beforeinput` and a script assignment do not.
Cloning copies the live control value as required by the existing form-control
clone behavior, but deliberately does not copy the user-agent autofill state.

The fixed WPT revision used by the repository has no `:autofill` test. The
behavior is therefore covered by native regression tests and compared against
the normative HTML and Selectors definitions. Reference-browser automation
cannot create a trusted autofill operation with the WebDriver commands
available in that revision, so no scripted Firefox fill result is claimed.
