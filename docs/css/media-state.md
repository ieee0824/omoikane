# Media state selectors

Omoikane stores the selector-visible state of HTML `audio` and `video` elements
in the native DOM. JavaScript publishes one owned bit set whenever playback,
seeking, loading, muting, UA volume lock, or Picture-in-Picture state changes.
The CSS matcher therefore does not execute author JavaScript or retain a
borrowed JavaScript object while resolving style.

The supported non-functional pseudo-classes are:

- `:playing` and `:paused`, derived from the media element's live paused flag;
- `:seeking`, set from assignment until the queued seek completes;
- `:buffering`, when playback is requested while the network is loading and no
  more than the current frame is ready;
- `:muted`, derived from the live `muted` state rather than the `muted` content
  attribute (`defaultMuted` reflects that attribute independently);
- `:volume-locked`, reserved for the UA/platform volume-lock provider and false
  when the provider has not locked volume;
- `:picture-in-picture`, set only on `document.pictureInPictureElement` while
  its Picture-in-Picture session is active.

All seven names are accepted by selector parsing and
`CSS.supports('selector(...)')`. Functional spellings such as `:paused()` are
invalid. SVG elements and arbitrary elements with similar attributes never
match.

Picture-in-Picture entry consumes transient user activation. Entering a second
video leaves the previous one, while `Document.exitPictureInPicture()` and
removal of the active video clear the native selector state before dispatching
the leave notification. `PictureInPictureWindow` is not author-constructible.

State publication invalidates the subject and the existing dependent-selector
cache, including `:has()` ancestors and sibling selectors. Clones start with a
fresh paused state and do not inherit a live Picture-in-Picture or seeking
session.

The pinned WPT checkout used by this repository currently contains the media
IDL files but no tests for these seven HTML media pseudo-classes. The dedicated
native regression suite consequently covers grammar, semantic matching,
dependent style invalidation, queued seek completion, playback event ordering,
and Picture-in-Picture activation/entry/exit.

Firefox 158.0.1 was also run headlessly against the same seven grammar cases,
fresh-video state, and the separation of `muted` from `defaultMuted`. All 29
boolean observations agree with Omoikane. The captured Firefox result is kept
at `.artifacts/issue1269/media-state-firefox-158.0.1.json` outside the source
tree. New upstream media-pseudo WPTs landed after this repository's fixed WPT
revision and are not presented as fixed-revision test results here.
