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
  git clone --depth 1 --filter=blob:none --no-checkout "$repo" "$destination"
fi
# Execute original sub/py fixtures over verified TLS for the HTTPS runner.
collect_pattern "/tools/localpaths.py" "/tools/wptserve/" "/tools/third_party/" "/tools/certs/"
collect_pattern "/tools/serve/" "/tools/manifest/" "/tools/gitignore/"
collect_pattern "/tools/metadata/"
collect_pattern "/service-workers/service-worker/resources/test-helpers.sub.js"
collect_pattern "/common/security-features/" "/trusted-types/navigate-to-javascript-url-006.sub.html"
collect_pattern "/content-security-policy/generic/"
collect_pattern "/content-security-policy/gen/top.meta/script-src-self/worklet-paint*.https.html*" "/content-security-policy/gen/top.meta/worker-src-none/worklet-paint*.https.html*"
collect_pattern "/content-security-policy/gen/top.http-rp/script-src-self/worklet-paint*.https.html*" "/content-security-policy/gen/top.http-rp/worker-src-none/worklet-paint*.https.html*"
collect_pattern \
  "/css/selectors/user-invalid.html" \
  "/css/selectors/user-valid.html" \
  "/css/selectors/invalidation/user-valid-user-invalid.html" \
  "/css/selectors/user-invalid-form-submission-invalidation.html" \
  "/css/selectors/user-invalid-form-submission-invalidation-ref.html" \
  "/css/selectors/user-valid-user-invalid-multifield-inputs.tentative.html"
collect_pattern "/resources/" "/html/resources/common.js" "/shadow-dom/resources/event-path-test-helpers.js" "/shadow-dom/Element-interface-attachShadow.html" "/shadow-dom/Element-interface-shadowRoot-attribute.html" "/shadow-dom/HTMLSlotElement-interface.html" "/shadow-dom/Slottable-mixin.html" "/shadow-dom/Extensions-to-Event-Interface.html" "/shadow-dom/event-inside-shadow-tree.html" "/shadow-dom/event-inside-slotted-node.html" "/custom-elements/registries/upgrade.html" "/custom-elements/connected-callbacks-template.html" "/dom/nodes/Element-childElement-null.html" "/dom/nodes/Element-childElementCount-nochild.html" "/dom/nodes/Node-isConnected.html" "/dom/nodes/Element-childElementCount.html" "/dom/nodes/Element-childElementCount-dynamic-add.html" "/dom/nodes/Element-childElementCount-dynamic-remove.html" "/dom/nodes/CharacterData-remove.html" "/dom/nodes/ChildNode-remove.js" "/dom/nodes/Element-remove.html" "/dom/nodes/DocumentType-remove.html" "/dom/nodes/Text-splitText.html" "/dom/nodes/CharacterData-data.html" "/dom/nodes/CharacterData-appendData.html" "/dom/nodes/CharacterData-substringData.html" "/dom/nodes/CharacterData-insertData.html" "/dom/nodes/CharacterData-deleteData.html" "/dom/nodes/CharacterData-replaceData.html" "/dom/nodes/CharacterData-surrogates.html" "/dom/nodes/Node-nodeValue.html" "/dom/nodes/Node-normalize.html" "/dom/nodes/Node-textContent.html" "/dom/nodes/MutationObserver-sanity.html" "/dom/nodes/MutationObserver-callback-arguments.html" "/dom/nodes/MutationObserver-takeRecords.html" "/dom/nodes/MutationObserver-disconnect.html" "/dom/nodes/mutationobservers.js" "/dom/nodes/MutationObserver-attributes.html" "/dom/nodes/MutationObserver-characterData.html"
collect_pattern "/css/css-overflow/parsing/" "/css/css-scroll-anchoring/parsing/" "/css/css-scrollbars/"
collect_pattern "/css/css-viewport/computedStyle-zoom.html" "/css/css-viewport/zoom/parsing/"
collect_pattern "/css/css-color-adjust/parsing/print-color-adjust.html" "/css/css-forced-color-adjust/parsing/"
collect_pattern "/css/css-shadow/shadow-cascade-order-001.html"
# Customizable select (#1573).
collect_pattern "/html/semantics/forms/the-select-element/customizable-select/" "/html/semantics/popovers/resources/"
# Close watchers, dialog closedby/requestClose and Invoker Commands (#1297).
collect_pattern "/close-watcher/" "/html/semantics/the-button-element/command-and-commandfor/" \
  "/html/semantics/forms/the-button-element/button-click-*-with-commandfor.html" \
  "/html/semantics/interactive-elements/the-dialog-element/dialog-closedby*" \
  "/html/semantics/interactive-elements/the-dialog-element/dialog-requestclose*" \
  "/html/semantics/interactive-elements/the-dialog-element/dialog-popover-closedby*" \
  "/html/semantics/interactive-elements/the-dialog-element/dialog-light-dismiss*" \
  "/html/semantics/forms/the-input-element/show-picker*" "/html/semantics/forms/the-input-element/resources/" \
  "/html/semantics/forms/the-input-element/input-types.js" "/common/top-layer.js" \
  "/html/semantics/popovers/resources/popover-utils.js"
collect_pattern "/css/css-highlight-api/"
collect_pattern "/requestidlecallback/" "/scheduler/" "/dom/abort/resources/"
# Keepalive Fetch APIs (#1294).
collect_pattern "/beacon/" "/fetch/fetch-later/"
# Picture-in-Picture IDL conformance (#1615).
collect_pattern "/picture-in-picture/" "/common/media.js" "/media/movie_5.mp4" "/media/movie_5.webm"
# TextTrack and WebVTT parsing/IDL conformance (#1614).
collect_pattern "/webvtt/" "/html/semantics/embedded-content/media-elements/track/"
# Media capture streams and deterministic MediaRecorder behavior (#1612).
collect_pattern "/mediacapture-streams/" "/mediacapture-record/"
# Media Source Extensions state, object URL, and SourceBuffer contracts (#1613).
collect_pattern "/media-source/"
# Media Session metadata, playback, position, and action-handler contracts (#1618).
collect_pattern "/mediasession/"
# Web Speech synthesis and recognition contracts (#1619).
collect_pattern "/speech-api/"
# WebCodecs data containers and provider-independent API contracts (#1616).
collect_pattern "/webcodecs/"
# OfflineAudioContext and AudioWorklet graph/runtime contracts (#1617).
collect_pattern "/webaudio/"
collect_pattern "/common/performance-timeline-utils.js" "/common/performance-timeline-utils.js.headers"
collect_pattern "/paint-timing/" "/largest-contentful-paint/" "/server-timing/" "/longtask-timing/" "/event-timing/" "/layout-instability/" "/images/"
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
# CSS Color 5 (#1277).
collect_pattern \
  "/css/css-color/parsing/color-computed-color-mix-function.html" \
  "/css/css-color/parsing/color-invalid-color-mix-function.html" \
  "/css/css-color/parsing/color-valid-color-mix-function.html" \
  "/css/css-color/parsing/color-computed-contrast-color-function.html" \
  "/css/css-color/parsing/color-invalid-contrast-color-function.html" \
  "/css/css-color/parsing/color-valid-contrast-color-function.html" \
  "/css/css-color/parsing/color-computed-relative-color.html" \
  "/css/css-color/parsing/color-invalid-relative-color.html" \
  "/css/css-color/parsing/color-valid-relative-color.html" \
  "/css/css-color/light-dark-basic.html" \
  "/css/css-color/light-dark-currentcolor-in-color.html" \
  "/css/css-color-adjust/parsing/color-scheme-computed.html" \
  "/css/css-color-adjust/parsing/color-scheme-invalid.html" \
  "/css/css-color-adjust/parsing/color-scheme-valid.html"
# CSSOM rule interfaces (#1280).
collect_pattern \
  "/css/css-animations/keyframes-rule-caching.html" \
  "/css/css-page/cssom/page-001.html" \
  "/css/css-page/cssom/margin-001.html" \
  "/css/css-view-transitions/navigation/at-rule-cssom.html"
# CSS Typed OM (#1282).
collect_pattern "/css/css-typed-om/"
collect_pattern "/css/css-logical/"
collect_pattern "/css/css-fonts/parsing/" "/css/css-fonts/font-variant-alternates-parsing.html" "/css/css-fonts/test_font_feature_values_parsing.html"
collect_pattern "/reporting/" "/content-security-policy/securitypolicyviolation/constructor-required-fields.html"
collect_pattern "/cookies/resources/"
collect_pattern \
  "/permissions-policy/policy-extends-to-sandbox.html" \
  "/permissions-policy/navigate-to-blob.sub.html" \
  "/permissions-policy/experimental-features/focus-without-user-activation-disabled.html" \
  "/permissions-policy/experimental-features/focus-without-user-activation-enabled.sub.html" \
  "/permissions-policy/experimental-features/resources/common.js" \
  "/permissions-policy/experimental-features/resources/focus-without-user-activation-iframe-tentative.html" \
  "/permissions-policy/resources/navigate-to-blob.html" \
  "/permissions-policy/resources/sandbox-self.html" \
  "/permissions-policy/resources/sandbox-self.html.headers" \
  "/permissions-policy/resources/nested-sandbox.html" \
  "/html/semantics/embedded-content/the-iframe-element/iframe-allow.html"
collect_pattern \
  "/trusted-types/support/" \
  "/trusted-types/DedicatedWorker-eval.html" \
  "/trusted-types/SharedWorker-eval.html" \
  "/trusted-types/DedicatedWorker-importScripts.html" \
  "/trusted-types/SharedWorker-importScripts.html" \
  "/trusted-types/DedicatedWorker-block-eval-function-constructor.html" \
  "/trusted-types/SharedWorker-block-eval-function-constructor.html" \
  "/trusted-types/trusted-types-reporting-for-DedicatedWorker-eval.html" \
  "/trusted-types/trusted-types-reporting-for-SharedWorker-eval.html" \
  "/trusted-types/should-trusted-type-policy-creation-be-blocked-by-csp-004-worker.html" \
  "/trusted-types/navigate-to-javascript-url-001.html" \
  "/trusted-types/navigate-to-javascript-url-002.html" \
  "/trusted-types/navigate-to-javascript-url-003.html" \
  "/trusted-types/navigate-to-javascript-url-004.html" \
  "/trusted-types/navigate-to-javascript-url-005.html" \
  "/trusted-types/navigate-to-javascript-url-008.html" \
  "/trusted-types/navigate-to-javascript-url-008.html.headers" \
  "/trusted-types/navigate-to-javascript-url-csp-headers.html" \
  "/trusted-types/Window-TrustedTypes.html" \
  "/trusted-types/TrustedTypePolicyFactory-createPolicy-unenforced.html" \
  "/trusted-types/TrustedTypePolicyFactory-isXXX.html" \
  "/trusted-types/TrustedTypePolicyFactory-createPolicy-cspTests.html" \
  "/trusted-types/trusted-types-reporting-for-Element-innerHTML.html" \
  "/trusted-types/trusted-types-reporting-for-DOMParser-parseFromString.html" \
  "/trusted-types/Window-block-eval-function-constructor.html" \
  "/trusted-types/eval-with-permissive-csp.html" \
  "/trusted-types/eval-csp-tt-default-policy.html" \
  "/trusted-types/eval-csp-tt-default-policy-mutate.html" \
  "/trusted-types/block-string-assignment-to-Window-setTimeout-setInterval.html" \
  "/trusted-types/block-string-assignment-to-Element-insertAdjacentHTML.html" \
  "/trusted-types/trusted-types-reporting-for-Window-eval.html" \
  "/trusted-types/trusted-types-reporting-for-Window-function-constructor.html" \
  "/trusted-types/trusted-types-reporting-for-Window-DedicatedWorker-constructor.html" \
  "/trusted-types/TrustedTypePolicyFactory-getPropertyType.tentative.html" \
  "/trusted-types/TrustedTypePolicyFactory-getAttributeType.html" \
  "/trusted-types/Window-setTimeout-setInterval.html" \
  "/trusted-types/block-string-assignment-to-Document-write.html" \
  "/trusted-types/block-string-assignment-to-Element-outerHTML.html" \
  "/trusted-types/block-string-assignment-to-Element-setAttribute.html" \
  "/trusted-types/block-string-assignment-to-Element-setAttributeNS.html" \
  "/trusted-types/block-string-assignment-to-HTMLIFrameElement-srcdoc.html" \
  "/trusted-types/block-string-assignment-to-Range-createContextualFragment.html" \
  "/trusted-types/trusted-types-reporting-for-HTMLScriptElement.html" \
  "/trusted-types/trusted-types-reporting-for-HTMLScriptElement-children-change.html" \
  "/trusted-types/block-string-assignment-to-Document-parseHTMLUnsafe.html" \
  "/trusted-types/block-string-assignment-to-Element-setHTMLUnsafe.html" \
  "/trusted-types/block-string-assignment-to-ShadowRoot-setHTMLUnsafe.html" \
  "/trusted-types/ServiceWorkerContainer-register.https.html" \
  "/trusted-types/ServiceWorker-eval.https.html" \
  "/trusted-types/ServiceWorker-importScripts.https.html" \
  "/trusted-types/TrustedType-AttributeNodes.html" \
  "/trusted-types/block-string-assignment-to-attribute-via-attribute-node.html" \
  "/trusted-types/block-string-assignment-to-ShadowRoot-innerHTML.html" \
  "/trusted-types/trusted-types-reporting-for-ShadowRoot-innerHTML.html" \
  "/trusted-types/Element-setAttribute-setAttributeNS-sinks.tentative.html"
collect_pattern \
  "/content-security-policy/script-src/worker-importscripts.sub.html" \
  "/content-security-policy/script-src/support/worker-with-script-src-none-importscripts.js" \
  "/content-security-policy/script-src/support/worker-with-script-src-none-importscripts.js.sub.headers" \
  "/content-security-policy/worker-src/dedicated-worker-src-script-fallback.sub.html" \
  "/content-security-policy/worker-src/shared-worker-src-script-fallback.sub.html" \
  "/content-security-policy/support/testharness-helper.js" \
  "/content-security-policy/support/dedicated-worker-helper.js" \
  "/content-security-policy/support/shared-worker-helper.js" \
  "/content-security-policy/support/ping.js" \
  "/content-security-policy/support/post-message.js"
collect_pattern \
  "/content-security-policy/script-src/script-src-strict_dynamic_discard_source_expressions.html" \
  "/content-security-policy/script-src/script-src-strict_dynamic_discard_source_expressions.html.headers" \
  "/content-security-policy/script-src/script-src-strict_dynamic_parser_inserted.html" \
  "/content-security-policy/script-src/script-src-strict_dynamic_parser_inserted.html.headers" \
  "/content-security-policy/script-src/script-src-strict_dynamic_parser_inserted_correct_nonce.html" \
  "/content-security-policy/script-src/script-src-strict_dynamic_parser_inserted_correct_nonce.html.headers" \
  "/content-security-policy/script-src/script-src-strict_dynamic_non_parser_inserted.html" \
  "/content-security-policy/script-src/script-src-strict_dynamic_non_parser_inserted.html.headers" \
  "/content-security-policy/script-src/script-src-strict_dynamic_non_parser_inserted_module.html" \
  "/content-security-policy/script-src/script-src-strict_dynamic_non_parser_inserted_module.html.headers" \
  "/content-security-policy/script-src/script-src-strict_dynamic_parser_inserted_module.html" \
  "/content-security-policy/script-src/script-src-strict_dynamic_parser_inserted_module.html.headers" \
  "/content-security-policy/script-src/script-src-strict_dynamic_double_policy_report_only.html" \
  "/content-security-policy/script-src/script-src-strict_dynamic_double_policy_report_only.html.headers" \
  "/content-security-policy/script-src/simpleSourcedScript.js" \
  "/content-security-policy/script-src/simpleSourcedModule.js"
collect_pattern \
  "/content-security-policy/unsafe-hashes/javascript_src_allowed-href.html" \
  "/content-security-policy/unsafe-hashes/javascript_src_denied_missing_unsafe_hashes-href.html" \
  "/content-security-policy/unsafe-hashes/javascript_src_denied_wrong_hash-href.html" \
  "/content-security-policy/unsafe-hashes/javascript_src_allowed-href_blank-script-src-elem.html" \
  "/content-security-policy/unsafe-hashes/support/helper.js"
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
collect_pattern "/css/css-text-decor/parsing/text-decoration-skip-ink-valid.html"
collect_pattern "/css/css-text-decor/parsing/text-decoration-skip-ink-invalid.html"
collect_pattern "/css/css-text-decor/parsing/text-decoration-skip-ink-computed.html"
collect_pattern "/css/css-text-decor/parsing/text-decoration-style-valid.html"
collect_pattern "/css/css-text-decor/parsing/text-decoration-style-invalid.html"
collect_pattern "/css/css-text-decor/parsing/text-decoration-style-computed.html"
collect_pattern "/css/css-text/parsing/text-indent-valid.html"
collect_pattern "/css/css-text/parsing/text-indent-invalid.html"
collect_pattern "/css/css-text/parsing/text-indent-computed.html"
collect_pattern "/css/css-text/parsing/word-spacing-valid.html"
collect_pattern "/css/css-text/parsing/word-spacing-invalid.html"
collect_pattern "/css/css-text/parsing/word-spacing-computed.html"
collect_pattern "/css/css-text/parsing/white-space-collapse-valid.html"
collect_pattern "/css/css-text/parsing/white-space-collapse-invalid.html"
collect_pattern "/css/css-text/parsing/white-space-collapse-computed.html"
collect_pattern "/css/css-text/parsing/text-wrap-valid.html"
collect_pattern "/css/css-text/parsing/text-wrap-invalid.html"
collect_pattern "/css/css-text/parsing/text-wrap-computed.html"
collect_pattern "/css/css-text/parsing/text-wrap-mode-valid.html"
collect_pattern "/css/css-text/parsing/text-wrap-mode-invalid.html"
collect_pattern "/css/css-text/parsing/text-wrap-mode-computed.html"
collect_pattern "/css/css-text/parsing/text-wrap-style-valid.html"
collect_pattern "/css/css-text/parsing/text-wrap-style-invalid.html"
collect_pattern "/css/css-text/parsing/text-wrap-style-computed.html"
collect_pattern "/css/css-text/parsing/line-break-valid.html"
collect_pattern "/css/css-text/parsing/line-break-invalid.html"
collect_pattern "/css/css-text/parsing/line-break-computed.html"
collect_pattern "/css/css-text/parsing/hyphens-valid.html"
collect_pattern "/css/css-text/parsing/hyphens-invalid.html"
collect_pattern "/css/css-text/parsing/hyphens-computed.html"
collect_pattern "/css/css-text/parsing/hyphenate-character-valid.html"
collect_pattern "/css/css-text/parsing/hyphenate-character-invalid.html"
collect_pattern "/css/css-text/parsing/hyphenate-character-computed.html"
collect_pattern "/css/css-text/parsing/tab-size-valid.html"
collect_pattern "/css/css-text/parsing/tab-size-invalid.html"
collect_pattern "/css/css-text/parsing/tab-size-computed.html"
collect_pattern "/css/css-ruby/parsing/ruby-align-valid.html"
collect_pattern "/css/css-ruby/parsing/ruby-align-invalid.html"
collect_pattern "/css/css-ruby/parsing/ruby-position-valid.html"
collect_pattern "/css/css-ruby/parsing/ruby-position-invalid.html"
collect_pattern "/css/css-text-decor/parsing/text-emphasis-computed.html"
collect_pattern "/css/css-text-decor/parsing/text-emphasis-position-valid.html"
collect_pattern "/css/css-text-decor/parsing/text-emphasis-position-invalid.html"
collect_pattern "/css/css-text-decor/parsing/text-emphasis-position-computed.html"
collect_pattern "/css/css-text-decor/parsing/text-emphasis-style-computed.html"
collect_pattern "/css/css-text-decor/parsing/text-emphasis-style-computed-vertical-lr.html"
collect_pattern "/css/css-text/parsing/text-autospace-valid.html"
collect_pattern "/css/css-anchor-position/parsing/anchor-scope-parsing.html"
collect_pattern "/css/css-anchor-position/parsing/position-try-fallbacks-parsing.html"
collect_pattern "/css/css-anchor-position/parsing/position-try-order-parsing.html"
collect_pattern "/css/css-anchor-position/parsing/position-visibility-parsing.html"
collect_pattern "/css/css-anchor-position/parsing/position-visibility-computed.html"
collect_pattern "/css/css-anchor-position/anchor-size-parse-valid.html"
collect_pattern "/css/css-anchor-position/anchor-size-parse-invalid.html"
collect_pattern "/css/css-anchor-position/at-position-try-cssom.html"
collect_pattern "/css/css-anchor-position/at-position-try-parse.html"
collect_pattern "/css/css-anchor-position/position-area-parsing.html"
collect_pattern "/css/css-text/parsing/text-autospace-invalid.html"
collect_pattern "/css/css-text/parsing/text-autospace-computed.html"
collect_pattern "/css/css-text/parsing/text-spacing-trim-valid.html"
collect_pattern "/css/css-text/parsing/text-spacing-trim-invalid.html"
collect_pattern "/css/css-text/parsing/text-spacing-trim-computed.html"
collect_pattern "/css/css-writing-modes/parsing/text-combine-upright-valid.html"
collect_pattern "/css/css-writing-modes/parsing/text-combine-upright-invalid.html"
collect_pattern "/css/css-writing-modes/parsing/text-combine-upright-computed.html"
collect_pattern "/css/css-writing-modes/parsing/text-orientation-valid.html"
collect_pattern "/css/css-writing-modes/parsing/text-orientation-invalid.html"
collect_pattern "/css/css-writing-modes/parsing/text-orientation-computed.html"
collect_pattern \
  "/css/css-values/round-mod-rem-computed.html" \
  "/css/css-values/round-mod-rem-invalid.html" \
  "/css/css-values/round-mod-rem-serialize.html" \
  "/css/css-values/hypot-pow-sqrt-computed.html" \
  "/css/css-values/hypot-pow-sqrt-invalid.html" \
  "/css/css-values/hypot-pow-sqrt-serialize.html" \
  "/css/css-values/exp-log-compute.html" \
  "/css/css-values/exp-log-invalid.html" \
  "/css/css-values/exp-log-serialize.html" \
  "/css/support/numeric-testcommon.js" \
  "/css/support/serialize-testcommon.js"
collect_pattern \
  "/css/css-conditional/container-queries/container-units-basic.html" \
  "/css/css-conditional/container-queries/container-units-auto.html" \
  "/css/css-conditional/container-queries/container-units-content-box.html" \
  "/css/css-conditional/container-queries/container-units-ineligible-container.html" \
  "/css/css-conditional/container-queries/container-units-invalidation.html" \
  "/css/css-conditional/container-queries/container-units-small-viewport-fallback.html" \
  "/css/css-conditional/container-queries/container-units-shadow.html" \
  "/css/css-conditional/container-queries/container-units-selection.html" \
  "/css/css-conditional/container-queries/support/cq-testcommon.js"
collect_pattern "/css/css-overflow/parsing/line-clamp-valid.html"
collect_pattern "/css/css-overflow/parsing/line-clamp-invalid.html"
collect_pattern \
  "/css/css-images/parsing/image-orientation-valid.html" \
  "/css/css-images/parsing/image-orientation-invalid.html" \
  "/css/css-images/parsing/image-orientation-computed.html" \
  "/css/css-images/image-orientation/image-orientation-from-image-computed-style.html" \
  "/css/css-images/image-orientation/image-orientation-none-computed-style.html" \
  "/css/css-images/image-orientation/support/" \
  "/css/css-images/image-set/image-set-parsing.html" \
  "/css/css-images/image-set/image-set-computed.sub.html" \
  "/css/css-images/cross-fade-computed-value.html" \
  "/css/compositing/parsing/background-blend-mode-valid.html" \
  "/css/compositing/parsing/background-blend-mode-invalid.html" \
  "/css/compositing/parsing/background-blend-mode-computed.html" \
  "/css/compositing/parsing/background-blend-mode-computed-multiple.html"
collect_pattern "/css/css-overflow/parsing/webkit-line-clamp-valid.html"
collect_pattern \
  "/css/css-backgrounds/parsing/border-image-valid.html" \
  "/css/css-backgrounds/parsing/border-image-invalid.html" \
  "/css/css-backgrounds/parsing/border-image-shorthand.sub.html" \
  "/css/css-backgrounds/parsing/border-image-source-valid.html" \
  "/css/css-backgrounds/parsing/border-image-source-invalid.html" \
  "/css/css-backgrounds/parsing/border-image-source-computed.sub.html" \
  "/css/css-backgrounds/parsing/border-image-slice-valid.html" \
  "/css/css-backgrounds/parsing/border-image-slice-invalid.html" \
  "/css/css-backgrounds/parsing/border-image-slice-computed.html" \
  "/css/css-backgrounds/parsing/border-image-width-valid.html" \
  "/css/css-backgrounds/parsing/border-image-width-invalid.html" \
  "/css/css-backgrounds/parsing/border-image-width-computed.html" \
  "/css/css-backgrounds/parsing/border-image-outset-valid.html" \
  "/css/css-backgrounds/parsing/border-image-outset-invalid.html" \
  "/css/css-backgrounds/parsing/border-image-outset-computed.html" \
  "/css/css-backgrounds/parsing/border-image-repeat-valid.html" \
  "/css/css-backgrounds/parsing/border-image-repeat-invalid.html" \
  "/css/css-backgrounds/parsing/border-image-repeat-computed.html" \
  "/css/support/parsing-testcommon.js"
collect_pattern "/css/css-overflow/parsing/webkit-line-clamp-invalid.html"
collect_pattern "/css/css-overflow/parsing/webkit-line-clamp-computed.html"
collect_pattern "/css/css-text/parsing/text-align-last-valid.html"
collect_pattern "/css/css-text/parsing/text-align-last-invalid.html"
collect_pattern "/css/css-text/parsing/text-align-last-computed.html"
collect_pattern "/css/css-text/parsing/text-justify-valid.html"
collect_pattern "/css/css-text/parsing/text-justify-invalid.html"
collect_pattern "/css/css-text/parsing/text-justify-computed.html"
collect_pattern "/css/css-text/parsing/text-justify-computed-legacy.html"
collect_pattern "/css/css-text/parsing/hanging-punctuation-valid.html"
collect_pattern "/css/css-text/parsing/hanging-punctuation-invalid.html"
collect_pattern "/css/css-inline/initial-letter/initial-letter-valid.html"
collect_pattern "/css/css-inline/initial-letter/initial-letter-invalid.html"
collect_pattern "/css/css-inline/initial-letter/initial-letter-computed.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/text-box-valid.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/text-box-invalid.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/text-box-computed.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/text-box-shorthand.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/text-box-trim-valid.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/text-box-trim-invalid.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/text-box-trim-computed.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/text-box-edge-valid.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/text-box-edge-invalid.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/text-box-edge-computed.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/inheritance.html"
collect_pattern "/css/css-inline/text-box-trim/parsing/values.js"
collect_pattern "/css/support/inheritance-testcommon.js"
collect_pattern "/css/css-text-decor/text-shadow/"
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
collect_pattern "/html/webappapis/scripting/processing-model-2/" "/cors/support.js"
collect_pattern "/compression/"
collect_pattern "/encoding/streams/" "/common/sab.js"
# Streams, Body and URL modern APIs (#1294).
collect_pattern \
  "/streams/resources/" \
  "/streams/readable-byte-streams/construct-byob-request.any.js" \
  "/streams/readable-streams/async-iterator.any.js" \
  "/streams/readable-streams/from.any.js" \
  "/FileAPI/blob/Blob-bytes.any.js" \
  "/fetch/api/body/formdata.any.js" \
  "/fetch/api/body/mime-type.any.js" \
  "/FileAPI/support/Blob.js" \
  "/url/url-statics-parse.any.js" \
  "/url/urlsearchparams-size.any.js" \
  "/urlpattern/"
collect_pattern "/css/selectors/is-where-error-recovery.html"
collect_pattern "/css/selectors/child-indexed-pseudo-class.html" "/css/selectors/nth-last-child-invalid.html" "/css/selectors/invalidation/nth-child-whole-subtree.html"
collect_pattern "/html/semantics/selectors/pseudo-classes/required-optional.html" "/html/semantics/selectors/pseudo-classes/required-optional-hidden.html" "/html/semantics/selectors/pseudo-classes/utils.js"
collect_pattern "/html/semantics/selectors/pseudo-classes/placeholder-shown-type-change.html"
collect_pattern "/html/semantics/selectors/pseudo-classes/readwrite-readonly.html" "/html/semantics/selectors/pseudo-classes/readwrite-readonly-type-change.html"
collect_pattern "/html/editing/editing-0/contenteditable/contenteditable-enumerated-ascii-case-insensitive.html" "/html/editing/editing-0/contenteditable/user-interaction-editing-contenteditable.html"
collect_pattern "/css/selectors/selector-read-write-type-change-001.html" "/css/selectors/selector-read-write-type-change-001-ref.html" "/css/selectors/selector-read-write-type-change-002.html" "/css/selectors/selector-read-write-type-change-002-ref.html"
collect_pattern "/custom-elements/pseudo-class-defined.html" "/custom-elements/pseudo-class-defined-customized-builtins.html"
collect_pattern "/custom-elements/registries/" "/custom-elements/resources/" "/common/blank.html" "/common/object-association.js"
collect_pattern "/custom-elements/state/" "/css/selectors/parsing/parse-state.html" "/common/gc.js"
collect_pattern "/html/semantics/selectors/pseudo-classes/inrange-outofrange.html" "/html/semantics/selectors/pseudo-classes/inrange-outofrange-type-change.html" "/html/semantics/selectors/pseudo-classes/inrange-outofrange-time-reversed.html" "/html/semantics/forms/the-input-element/number.html" "/html/semantics/forms/the-input-element/range.html" "/html/semantics/forms/the-input-element/range-2.html" "/html/semantics/forms/the-input-element/input-valueasnumber.html" "/html/semantics/forms/the-input-element/datetime-local.html"
collect_pattern "/html/semantics/selectors/pseudo-classes/default.html"
collect_pattern "/html/semantics/selectors/pseudo-classes/indeterminate.html" "/html/semantics/selectors/pseudo-classes/indeterminate-radio.html" "/html/semantics/selectors/pseudo-classes/indeterminate-type-change.html" "/html/semantics/selectors/pseudo-classes/checked-indeterminate.window.js" "/html/semantics/forms/the-input-element/radio.html" "/html/semantics/forms/the-input-element/radio-input-cancel.html"
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
collect_pattern "/css/css-conditional/js/CSS-supports-L4.html"
collect_pattern "/css/css-conditional/js/CSS-supports-selector-detecting-invalid-in-logical-combinations.html"
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
collect_pattern "/css/cssom-view/MediaQueryList-extends-EventTarget-interop.html" "/css/cssom-view/MediaQueryListEvent.html" "/css/cssom-view/resources/matchMedia.js"
collect_pattern "/css/mediaqueries/mq-calc-resolution.html" "/css/mediaqueries/mq-range-001.html" "/css/mediaqueries/mq-calc-001.html" "/css/mediaqueries/mq-calc-002.html" "/css/mediaqueries/mq-calc-003.html" "/css/mediaqueries/mq-calc-004.html" "/css/mediaqueries/mq-calc-005.html" "/css/mediaqueries/mq-calc-006.html" "/css/mediaqueries/mq-calc-007.html" "/css/mediaqueries/mq-calc-008.html" "/css/mediaqueries/aspect-ratio-001.html" "/css/mediaqueries/aspect-ratio-002.html" "/css/mediaqueries/aspect-ratio-003.html" "/css/mediaqueries/aspect-ratio-004.html" "/css/mediaqueries/aspect-ratio-005.html" "/css/mediaqueries/aspect-ratio-006.html" "/css/mediaqueries/mq-negative-range-001.html" "/css/mediaqueries/mq-negative-range-002.html" "/css/reference/ref-filled-green-100px-square.xht"
collect_pattern "/css/mediaqueries/dynamic-range.html" "/css/mediaqueries/forced-colors.html" "/css/mediaqueries/inverted-colors.html" "/css/mediaqueries/prefers-color-scheme.html" "/css/mediaqueries/prefers-contrast.html" "/css/mediaqueries/prefers-reduced-data.html" "/css/mediaqueries/prefers-reduced-motion.html" "/css/mediaqueries/prefers-reduced-transparency.html" "/css/mediaqueries/scripting.html" "/css/mediaqueries/update-media-feature.html" "/css/mediaqueries/overflow-media-features.html" "/css/mediaqueries/display-mode.html" "/css/mediaqueries/resources/matchmedia-utils.js"
collect_pattern "/css/css-ui/accent-color-parsing.html" "/css/css-ui/accent-color-computed.html"
collect_pattern "/css/css-will-change/parsing/will-change-valid.html" "/css/css-will-change/parsing/will-change-invalid.html" "/css/css-will-change/parsing/will-change-computed.html"
collect_pattern "/css/css-transforms/individual-transform/" "/css/reference/ref-filled-green-200px-square.html" "/css/reference/ref-filled-green-100px-square.xht"
collect_pattern "/css/css-transforms/parsing/translate-parsing-valid.html"
collect_pattern "/css/css-transforms/parsing/translate-parsing-invalid.html"
collect_pattern "/css/css-transforms/parsing/translate-parsing-computed.html"
collect_pattern "/css/css-transforms/parsing/rotate-parsing-valid.html"
collect_pattern "/css/css-transforms/parsing/rotate-parsing-invalid.html"
collect_pattern "/css/css-transforms/parsing/rotate-parsing-computed.html"
collect_pattern "/css/css-transforms/parsing/scale-parsing-valid.html"
collect_pattern "/css/css-transforms/parsing/scale-parsing-invalid.html"
collect_pattern "/css/css-transforms/parsing/scale-parsing-computed.html"
collect_pattern \
  "/css/css-color/parsing/color-invalid.html" \
  "/css/css-color/parsing/color-invalid-rgb.html" \
  "/css/css-color/parsing/color-invalid-hsl.html"
collect_pattern \
  "/css/support/parsing-testcommon.js" \
  "/css/support/color-testcommon.js" \
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
collect_pattern \
  "/css/css-animations/KeyframeEffect-getKeyframes.tentative.html" \
  "/css/css-animations/KeyframeEffect-setKeyframes.tentative.html" \
  "/css/css-animations/KeyframeEffect-target.tentative.html" \
  "/css/css-animations/support/testcommon.js" \
  "/css/css-animations/parsing/animation-composition-valid.tentative.html" \
  "/css/css-animations/parsing/animation-composition-invalid.tentative.html" \
  "/css/css-transitions/parsing/transition-behavior.html" \
  "/scroll-animations/scroll-timelines/constructor.html" \
  "/scroll-animations/scroll-timelines/testcommon.js"
collect_pattern "/custom-elements/form-associated/" "/html/semantics/forms/form-submission-0/resources/targetted-form.js"
collect_pattern "/common/blank.html" "/FileAPI/file/resources/echo-content-escaped.py"
collect_pattern \
  "/html/cross-origin-embedder-policy/meta-http-equiv.https.html" \
  "/html/cross-origin-embedder-policy/non-initial-about-blank.https.html" \
  "/html/cross-origin-embedder-policy/non-initial-about-blank.https.html.headers" \
  "/html/cross-origin-embedder-policy/require-corp-about-blank.https.html" \
  "/html/cross-origin-embedder-policy/require-corp-about-blank.https.html.headers"
collect_pattern "/domxpath/"
collect_pattern "/visual-viewport/"
collect_pattern "/page-visibility/"
collect_pattern "/web-locks/"
collect_pattern "/cookiestore/"
collect_pattern "/storage-access-api/"
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
  "/css/css-lists/parsing/counter-increment-invalid.html" \
  "/css/css-lists/parsing/counter-set-valid.html" \
  "/css/css-lists/parsing/counter-set-invalid.html" \
  "/css/css-lists/parsing/counter-set-computed.html"
collect_pattern \
  "/css/css-shapes/parsing/shape-margin-valid.html" \
  "/css/css-shapes/parsing/shape-margin-invalid.html" \
  "/css/css-shapes/parsing/shape-margin-computed.html" \
  "/css/support/parsing-testcommon.js" \
  "/css/support/computed-testcommon.js"
# CSS Masking/Fill and Stroke coverage for Issue #1552.
collect_pattern \
  "/css/css-masking/parsing/clip-path-valid.html" \
  "/css/css-masking/parsing/clip-path-invalid.html" \
  "/css/css-masking/parsing/clip-path-computed.html" \
  "/css/css-masking/parsing/clip-path-shape-parsing.html" \
  "/svg/painting/parsing/paint-order-valid.svg" \
  "/svg/painting/parsing/paint-order-invalid.svg" \
  "/svg/painting/parsing/paint-order-computed.svg"
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
collect_pattern "/interfaces/" "/html/dom/" "/mathml/idlharness.window.js" "/html/semantics/forms/the-meter-element/meter.html" "/html/semantics/forms/the-meter-element/meter-leading-plus-sign.html" "/html/semantics/forms/the-progress-element/progress.html" "/html/semantics/forms/the-progress-element/progress-2.html" "/html/semantics/forms/the-progress-element/progress-max-setting.html" "/html/semantics/forms/the-progress-element/progress.window.js" "/html/semantics/forms/the-progress-element/serialization-regression-test.html" "/html/semantics/forms/form-submission-0/SubmitEvent.window.js" "/html/semantics/forms/form-submission-0/FormDataEvent.window.js" "/touch-events/" "/dom/events/Body-FrameSet-Event-Handlers.html" "/dom/events/event-global-set-before-handleEvent-lookup.window.js" "/dom/events/window-event-restored-after-throwing-onerror.html" "/dom/events/event-handler-attribute-replace-preserves-passive.html" "/dom/events/Event-dispatch-handlers-changed.html" "/common/utils.js"
collect_pattern "/inert/"
collect_pattern "/uievents/constructors/inputevent-constructor.html"
collect_pattern "/clipboard-apis/clipboard-events-synthetic.html" "/clipboard-apis/clipboard-item.https.html" "/clipboard-apis/events/copy-event.html"
collect_pattern "/common/subset-tests-by-key.js" "/common/get-host-info.sub.js"
collect_pattern "/common/redirect.py"
collect_pattern "/common/dispatcher/" "/html/cross-origin-opener-policy/"
collect_pattern "/css/cssom/caretPositionFromPoint*.html" "/shadow-dom/Document-caretPositionFromPoint.tentative.html"
collect_pattern "/css/cssom-view/range-client-rects-surrogate-indexing.html" "/css/cssom-view/range-bounding-client-rect*.html"
collect_pattern "/fonts/noto/NotoNaskhArabic-regular.woff2"
current_revision="$(git_wpt rev-parse HEAD 2>/dev/null || true)"
collect_pattern \
  "/css/css-flexbox/parsing/order-valid.html" \
  "/css/css-flexbox/parsing/order-invalid.html" \
  "/css/css-flexbox/parsing/order-computed.html" \
  "/css/css-flexbox/hittest-overlapping-order.html" \
  "/css/css-grid/parsing/grid-auto-flow-valid.html" \
  "/css/css-grid/parsing/grid-auto-flow-invalid.html" \
  "/css/css-grid/parsing/grid-auto-flow-computed.html" \
  "/css/css-grid/parsing/grid-auto-rows-valid.html" \
  "/css/css-grid/parsing/grid-auto-rows-invalid.html" \
  "/css/css-grid/parsing/grid-auto-rows-computed.html" \
  "/css/css-grid/parsing/grid-auto-columns-valid.html" \
  "/css/css-grid/parsing/grid-auto-columns-invalid.html" \
  "/css/css-grid/parsing/grid-auto-columns-computed.html"
collect_pattern \
  "/css/geometry/" \
  "/dom/collections/" \
  "/dom/ranges/StaticRange-constructor.html" \
  "/dom/interface-objects.html" \
  "/selection/shadow-dom/tentative/"
collect_pattern \
  "/css/selectors/open-pseudo.html" \
  "/css/selectors/invalidation/open-pseudo-class-in-has.html"
collect_pattern \
  "/css/css-view-transitions/active-view-transition-on-non-root.html" \
  "/css/css-view-transitions/active-view-transition-pseudo-class-match.html" \
  "/css/css-view-transitions/document-active-view-transition.html" \
  "/css/css-view-transitions/duplicate-tag-rejects-capture.html" \
  "/css/css-view-transitions/pseudo-computed-style-stays-in-sync-with-new-element.html" \
  "/css/css-view-transitions/view-transition-types-one-green-square-ref.html" \
  "/common/reftest-wait.js" \
  "/css/css-view-transitions/support/"
# Navigation API and View Transitions (#1283).
collect_pattern \
  "/navigation-api/navigation-history-entry/current-basic.html" \
  "/navigation-api/navigation-history-entry/resources/is_uuid.js" \
  "/navigation-api/currententrychange-event/constructor.html" \
  "/navigation-api/currententrychange-event/history-pushState.html" \
  "/navigation-api/currententrychange-event/history-replaceState.html" \
  "/navigation-api/currententrychange-event/navigation-updateCurrentEntry.html" \
  "/navigation-api/navigate-event/event-constructor.html" \
  "/navigation-api/navigation-methods/navigate-info-and-state.html"
# Canvas 2D drawing (#1292).
collect_pattern "/html/canvas/element/" "/html/canvas/resources/" "/images/" "/fonts/CanvasTest*.ttf" "/fonts/Lato-Medium.ttf" "/fonts/pass.woff" "/fonts/fail.woff"
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
