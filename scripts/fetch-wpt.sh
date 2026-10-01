#!/usr/bin/env bash
set -euo pipefail
repo="https://github.com/web-platform-tests/wpt.git"
revision="$(tr -d "[:space:]" < tests/wpt/revision.txt)"
destination="$(python3 -c 'import pathlib, sys; print(pathlib.Path(sys.argv[1]).resolve())' "${WPT_ROOT:-target/wpt}")"
patterns=()
git_wpt() { git -c safe.directory="$destination" -C "$destination" "$@"; }
collect_pattern() { patterns+=("$@"); }
if [[ ! -d "$destination/.git" ]]; then
  mkdir -p "$(dirname "$destination")"
  git clone --filter=blob:none --no-checkout "$repo" "$destination"
fi
collect_pattern "/resources/" "/html/resources/common.js" "/shadow-dom/resources/event-path-test-helpers.js" "/shadow-dom/Element-interface-attachShadow.html" "/shadow-dom/Element-interface-shadowRoot-attribute.html" "/shadow-dom/HTMLSlotElement-interface.html" "/shadow-dom/Slottable-mixin.html" "/shadow-dom/Extensions-to-Event-Interface.html" "/shadow-dom/event-inside-shadow-tree.html" "/shadow-dom/event-inside-slotted-node.html" "/custom-elements/registries/upgrade.html" "/custom-elements/connected-callbacks-template.html" "/dom/nodes/Element-childElement-null.html" "/dom/nodes/Element-childElementCount-nochild.html" "/dom/nodes/Node-isConnected.html" "/dom/nodes/Element-childElementCount.html" "/dom/nodes/Element-childElementCount-dynamic-add.html" "/dom/nodes/Element-childElementCount-dynamic-remove.html" "/dom/nodes/CharacterData-remove.html" "/dom/nodes/ChildNode-remove.js" "/dom/nodes/Element-remove.html" "/dom/nodes/DocumentType-remove.html" "/dom/nodes/Text-splitText.html" "/dom/nodes/CharacterData-data.html" "/dom/nodes/CharacterData-appendData.html" "/dom/nodes/CharacterData-substringData.html" "/dom/nodes/CharacterData-insertData.html" "/dom/nodes/CharacterData-deleteData.html" "/dom/nodes/CharacterData-replaceData.html" "/dom/nodes/CharacterData-surrogates.html" "/dom/nodes/Node-nodeValue.html" "/dom/nodes/Node-normalize.html" "/dom/nodes/Node-textContent.html" "/dom/nodes/MutationObserver-sanity.html" "/dom/nodes/MutationObserver-callback-arguments.html" "/dom/nodes/MutationObserver-takeRecords.html" "/dom/nodes/MutationObserver-disconnect.html" "/dom/nodes/mutationobservers.js" "/dom/nodes/MutationObserver-attributes.html" "/dom/nodes/MutationObserver-characterData.html"
collect_pattern "/css/css-shadow/shadow-cascade-order-001.html"
collect_pattern \
  "/css/css-color/parsing/color-computed-color-function.html" \
  "/css/css-color/parsing/color-computed-hwb.html" \
  "/css/css-color/parsing/color-computed-lab.html" \
  "/css/css-color/parsing/color-invalid-color-function.html" \
  "/css/css-color/parsing/color-invalid-hwb.html" \
  "/css/css-color/parsing/color-invalid-lab.html" \
  "/css/css-color/parsing/color-valid-color-function.html" \
  "/css/css-color/parsing/color-valid-hwb.html" \
  "/css/css-color/parsing/color-valid-lab.html"
collect_pattern "/css/css-logical/"
collect_pattern \
  "/dom/events/AddEventListenerOptions-once.any.js" \
  "/dom/events/AddEventListenerOptions-passive.any.js" \
  "/dom/events/AddEventListenerOptions-signal.any.js"
collect_pattern \
  "/dom/nodes/attributes-namednodemap.html" \
  "/dom/nodes/attributes-namednodemap-cross-document.window.js" \
  "/dom/nodes/attributes.html" \
  "/dom/nodes/attributes.js" \
  "/dom/nodes/productions.js"
collect_pattern "/css/css-text-decor/parsing/text-underline-position-valid.html"
collect_pattern "/css/css-text-decor/parsing/text-underline-position-invalid.html"
collect_pattern "/css/css-text-decor/parsing/text-underline-position-computed.html"
collect_pattern "/css/css-text-decor/text-underline-offset-valid.html"
collect_pattern "/css/css-text-decor/text-underline-offset-invalid.html"
collect_pattern "/css/css-text-decor/text-underline-offset-computed.html"
collect_pattern "/css/css-text-decor/text-underline-offset-initial.html"
collect_pattern \
  "/webmessaging/Channel_postMessage_event_properties.any.js" \
  "/webmessaging/Channel_postMessage_DataCloneErr.any.js" \
  "/webmessaging/MessageEvent.any.js" \
  "/webmessaging/postMessage_invalid_targetOrigin.htm" \
  "/webmessaging/postMessage_Document.htm" \
  "/webmessaging/postMessage_Function.htm" \
  "/webmessaging/postMessage_dup_transfer_objects.htm" \
  "/webmessaging/postMessage_solidus_sorigin.htm" \
  "/webmessaging/postMessage_MessagePorts_sorigin.htm" \
  "/webmessaging/support/ChildWindowPostMessage.htm" \
  "/workers/Worker_basic.htm" \
  "/workers/support/WorkerBasic.js" \
  "/css/css-shadow/part/support/shadow-helper.js" \
  "/css/css-shadow/part/host-part-001.html" \
  "/css/css-shadow/part/exportparts-multiple.html" \
  "/css/css-shadow/part/both-part-and-exportparts.html" \
  "/css/css-shadow/part/simple-inline.html" \
  "/css/css-shadow/part/simple-important-inline.html" \
  "/css/css-shadow/part/simple-important-important.html" \
  "/css/css-shadow/part/invalidation-change-part-name.html" \
  "/css/css-shadow/part/invalidation-change-exportparts-forward.html"
collect_pattern "/compression/"
collect_pattern "/encoding/streams/" "/common/sab.js"
collect_pattern "/css/selectors/is-where-error-recovery.html"
collect_pattern "/css/selectors/has-matches-to-uninserted-elements.html"
collect_pattern \
  "/css/selectors/invalidation/any-link-pseudo.html" \
  "/css/selectors/invalidation/target-pseudo-in-has.html" \
  "/html/browsers/browsing-the-web/scroll-to-fragid/target-pseudo-after-reinsertion.html"
collect_pattern \
  "/css/selectors/focus-visible-script-focus-001.html" \
  "/css/selectors/focus-within-focus-move.html" \
  "/css/selectors/focus-within-removal.html"
collect_pattern "/css/css-conditional/js/CSS-supports-L3.html"
collect_pattern "/css/css-conditional/js/supports-conditionText.html"
collect_pattern "/css/css-nesting/"
collect_pattern "/css/css-cascade/scope-cssom.html"
collect_pattern "/css/css-cascade/scope-media.html"
collect_pattern "/css/css-cascade/scope-supports.html"
collect_pattern \
  "/css/css-cascade/layer-basic.html" \
  "/css/css-cascade/layer-font-face-override.html" \
  "/css/css-cascade/layer-important.html" \
  "/css/css-cascade/layer-keyframes-override.html" \
  "/css/css-cascade/layer-vs-inline-style.html" \
  "/css/css-cascade/layer-rules-cssom.html" \
  "/css/css-cascade/parsing/layer.html" \
  "/css/css-cascade/parsing/all-valid.html" \
  "/css/css-cascade/parsing/all-invalid.html" \
  "/css/css-cascade/all-prop-revert-layer.html" \
  "/css/css-cascade/layer-import.html" \
  "/css/css-cascade/parsing/layer-import-parsing.html" \
  "/css/css-cascade/parsing/supports-import-parsing.html"
collect_pattern "/fonts/Ahem.ttf" "/fonts/noto/noto-sans-v8-latin-regular.woff"
collect_pattern \
  "/fonts/ahem.css" \
  "/css/css-values/ch-recalc-on-font-load.html" \
  "/css/css-values/cap-invalidation.html" \
  "/css/css-values/rlh-invalidation.html"
collect_pattern "/css/css-variables/variable-substitution-shorthands.html"
collect_pattern "/css/css-cascade/layer-counter-style-override.html"
collect_pattern "/css/css-properties-values-api/register-property.html" "/css/css-properties-values-api/register-property-syntax-parsing.html" "/css/css-properties-values-api/at-property-cssom.html" "/css/css-properties-values-api/determine-registration.html" "/css/css-properties-values-api/registered-properties-inheritance.html" "/css/css-properties-values-api/registered-property-cssom.html" "/css/css-properties-values-api/resources/utils.js"
collect_pattern "/css/cssom/CSSContainerRule.tentative.html"
collect_pattern \
  "/css/css-font-loading/fontfaceset-has.html" \
  "/css/css-font-loading/fontfaceset-clear-css-connected.html" \
  "/css/css-font-loading/fontfaceset-delete-css-connected.html" \
  "/css/css-font-loading/fontfaceset-update-after-stylesheet-change.html" \
  "/css/css-font-loading/fontfacesetloadevent-constructor.html" \
  "/css/css-font-loading/font-face-reject.html" \
  "/css/css-font-loading/resources/Rochester.otf" \
  "/css/css-font-loading/resources/GenR102.woff2"
collect_pattern \
  "/domparsing/createContextualFragment.html" \
  "/domparsing/XMLSerializer-serializeToString.html" \
  "/domparsing/xml-parse-serialize-roundtrip.html"
collect_pattern \
  "/html/semantics/popovers/toggleevent-interface.html" \
  "/html/semantics/popovers/togglePopover.html"
collect_pattern \
  "/fullscreen/api/document-fullscreen-enabled.html" \
  "/fullscreen/api/document-onfullscreenerror.html" \
  "/fullscreen/api/historical.html" \
  "/fullscreen/rendering/fullscreen-pseudo-class-support.html"
collect_pattern "/pointerlock/constructor.html" "/pointerlock/pointerlock_without_gesture.html"
collect_pattern \
  "/html/semantics/forms/constraints/support/validator.js" \
  "/html/semantics/forms/constraints/form-validation-checkValidity.html" \
  "/html/semantics/forms/constraints/form-validation-reportValidity.html" \
  "/html/semantics/forms/constraints/form-validation-validate.html" \
  "/html/semantics/forms/constraints/form-validation-validity-customError.html" \
  "/html/semantics/forms/constraints/form-validation-validity-patternMismatch.html" \
  "/html/semantics/forms/constraints/form-validation-validity-rangeOverflow.html" \
  "/html/semantics/forms/constraints/form-validation-validity-rangeUnderflow.html" \
  "/html/semantics/forms/constraints/form-validation-validity-stepMismatch.html" \
  "/html/semantics/forms/constraints/form-validation-validity-tooLong.html" \
  "/html/semantics/forms/constraints/form-validation-validity-tooShort.html" \
  "/html/semantics/forms/constraints/form-validation-validity-typeMismatch.html" \
  "/html/semantics/forms/constraints/form-validation-validity-valid.html" \
  "/html/semantics/forms/constraints/form-validation-validity-valueMissing.html" \
  "/html/semantics/forms/constraints/form-validation-willValidate.html" \
  "/html/semantics/forms/constraints/radio-group-valueMissing.html"
collect_pattern "/css/css-transforms/transform-getBoundingClientRect-001.html"
collect_pattern \
  "/css/css-color/parsing/color-invalid.html" \
  "/css/css-color/parsing/color-invalid-rgb.html" \
  "/css/css-color/parsing/color-invalid-hsl.html"
collect_pattern \
  "/css/support/parsing-testcommon.js" \
  "/css/support/computed-testcommon.js" \
  "/css/css-text-decor/text-decoration-thickness-valid.html" \
  "/css/css-text-decor/text-decoration-thickness-invalid.html" \
  "/css/css-text-decor/text-decoration-thickness-computed.html" \
  "/css/css-overflow/parsing/text-overflow-invalid.html" \
  "/css/css-transitions/parsing/transition-property-valid.html" \
  "/css/css-transitions/parsing/transition-property-invalid.html" \
  "/css/css-transitions/parsing/transition-property-computed.html" \
  "/css/css-transitions/parsing/transition-duration-valid.html" \
  "/css/css-transitions/parsing/transition-duration-invalid.html" \
  "/css/css-transitions/parsing/transition-delay-valid.html" \
  "/css/css-transitions/parsing/transition-delay-invalid.html" \
  "/css/css-transitions/parsing/transition-valid.html" \
  "/css/css-transitions/parsing/transition-invalid.html" \
  "/css/css-transitions/parsing/transition-computed.html" \
  "/css/css-transitions/support/helper.js" \
  "/css/css-transitions/events-001.html" \
  "/css/css-transitions/events-002.html"
collect_pattern "/custom-elements/form-associated/" "/html/semantics/forms/form-submission-0/resources/targetted-form.js"
collect_pattern "/common/blank.html" "/FileAPI/file/resources/echo-content-escaped.py"
collect_pattern "/domxpath/"
collect_pattern "/visual-viewport/"
collect_pattern "/page-visibility/"
collect_pattern "/web-locks/"
collect_pattern "/css/css-multicol/parsing/" "/css/support/shorthand-testcommon.js"
collect_pattern \
  "/css/css-overscroll-behavior/parsing/overscroll-behavior-valid.html" \
  "/css/css-overscroll-behavior/parsing/overscroll-behavior-invalid.html" \
  "/css/css-overscroll-behavior/parsing/overscroll-behavior-computed.html" \
  "/css/cssom-view/parsing/scroll-behavior-valid.html" \
  "/css/cssom-view/parsing/scroll-behavior-invalid.html" \
  "/css/cssom-view/parsing/scroll-behavior-computed.html" \
  "/css/css-scroll-snap/scroll-snap-type.html" \
  "/css/css-scroll-snap/scrollTo-scrollBy-snaps.html" \
  "/css/css-scroll-snap/scroll-padding-and-margin.html"
collect_pattern \
  "/css/cssom-view/elementFromPoint-001.html" \
  "/css/cssom-view/elementFromPoint-parameters.html" \
  "/css/cssom-view/elementsFromPoint-invalid-cases.html" \
  "/css/cssom-view/elementsFromPoint-shadowroot.html" \
  "/css/cssom-view/elementsFromPoint-iframes.html" \
  "/css/cssom-view/resources/elementsFromPoint.js" \
  "/css/cssom-view/resources/iframe1.html" \
  "/css/cssom-view/resources/iframe2.html"
collect_pattern "/css/css-break/parsing/"
collect_pattern \
  "/css/css-contain/content-visibility/parsing/" \
  "/css/css-contain/content-visibility/content-visibility-hidden-boundingbox-query.html" \
  "/css/css-contain/content-visibility/content-visibility-auto-relevancy-updates.html" \
  "/css/css-contain/content-visibility/content-visibility-050.html" \
  "/css/css-contain/content-visibility/content-visibility-forced-layout-client-rects.html" \
  "/css/css-contain/content-visibility/content-visibility-hidden-offsetTop-left-width-height.html" \
  "/css/css-contain/content-visibility/content-visibility-hidden-scrollTop-left-width-height.html" \
  "/css/css-contain/content-visibility/content-visibility-size-containment-001.html" \
  "/common/rendering-utils.js" \
  "/css/css-sizing/contain-intrinsic-size/parsing/"
collect_pattern \
  "/css/css-lists/parsing/counter-reset-invalid.html" \
  "/css/css-lists/parsing/counter-increment-invalid.html"
collect_pattern \
  "/css/css-shapes/parsing/shape-margin-valid.html" \
  "/css/css-shapes/parsing/shape-margin-invalid.html" \
  "/css/css-shapes/parsing/shape-margin-computed.html" \
  "/css/support/parsing-testcommon.js" \
  "/css/support/computed-testcommon.js"
collect_pattern \
  "/storage/storagemanager-estimate.https.any.js" \
  "/storage/storagemanager-persisted.https.any.js" \
  "/storage/storagemanager-persist.https.window.js" \
  "/storage/storagemanager-persist-persisted-match.https.window.js" \
  "/storage/resources/helpers.js"
collect_pattern \
  "/css/css-page/layers-001-print.html" \
  "/css/css-page/layers-001-print-ref.html" \
  "/css/css-page/layers-002-print.html" \
  "/css/css-page/layers-002-print-ref.html" \
  "/css/css-page/layers-003-print.html" \
  "/css/css-page/layers-003-print-ref.html" \
  "/css/css-page/layers-004-print.html" \
  "/css/css-page/layers-004-print-ref.html" \
  "/css/css-page/page-name-and-break-001-print.html" \
  "/css/css-page/page-name-and-break-002-print.html" \
  "/css/css-page/page-name-and-break-003-print.html" \
  "/css/css-page/page-name-and-break-004-print.html" \
  "/css/css-page/page-name-and-break-print-ref.html"
current_revision="$(git_wpt rev-parse HEAD 2>/dev/null || true)"
# A pinned revision alone does not prove that newly requested WPT paths exist.
if [[ "$current_revision" == "$revision" ]] &&
  diff -q <(printf '%s\n' "${patterns[@]}" | LC_ALL=C sort -u) \
    <(git_wpt sparse-checkout list 2>/dev/null | LC_ALL=C sort -u) >/dev/null; then
  exit 0
fi
git_wpt sparse-checkout init --no-cone
git_wpt sparse-checkout set "${patterns[@]}"
if [[ "$current_revision" != "$revision" ]]; then
  git_wpt fetch --depth 1 origin "$revision"
  git_wpt checkout --detach "$revision"
fi
