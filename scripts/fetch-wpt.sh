#!/usr/bin/env bash
set -euo pipefail
repo="https://github.com/web-platform-tests/wpt.git"
revision="$(tr -d "[:space:]" < tests/wpt/revision.txt)"
destination="$(realpath -m "${WPT_ROOT:-target/wpt}")"
git_wpt() { git -c safe.directory="$destination" -C "$destination" "$@"; }
if [[ ! -d "$destination/.git" ]]; then
  mkdir -p "$(dirname "$destination")"
  git clone --filter=blob:none --no-checkout "$repo" "$destination"
fi
git_wpt sparse-checkout init --no-cone
git_wpt sparse-checkout set "/resources/" "/html/resources/common.js" "/shadow-dom/resources/event-path-test-helpers.js" "/shadow-dom/Element-interface-attachShadow.html" "/shadow-dom/Element-interface-shadowRoot-attribute.html" "/shadow-dom/HTMLSlotElement-interface.html" "/shadow-dom/Slottable-mixin.html" "/shadow-dom/Extensions-to-Event-Interface.html" "/shadow-dom/event-inside-shadow-tree.html" "/shadow-dom/event-inside-slotted-node.html" "/custom-elements/registries/upgrade.html" "/custom-elements/connected-callbacks-template.html" "/dom/nodes/Element-childElement-null.html" "/dom/nodes/Element-childElementCount-nochild.html" "/dom/nodes/Node-isConnected.html" "/dom/nodes/Element-childElementCount.html" "/dom/nodes/Element-childElementCount-dynamic-add.html" "/dom/nodes/Element-childElementCount-dynamic-remove.html" "/dom/nodes/CharacterData-remove.html" "/dom/nodes/ChildNode-remove.js" "/dom/nodes/Element-remove.html" "/dom/nodes/DocumentType-remove.html" "/dom/nodes/Text-splitText.html" "/dom/nodes/CharacterData-data.html" "/dom/nodes/CharacterData-appendData.html" "/dom/nodes/CharacterData-substringData.html" "/dom/nodes/CharacterData-insertData.html" "/dom/nodes/CharacterData-deleteData.html" "/dom/nodes/CharacterData-replaceData.html" "/dom/nodes/CharacterData-surrogates.html" "/dom/nodes/Node-nodeValue.html" "/dom/nodes/Node-normalize.html" "/dom/nodes/Node-textContent.html" "/dom/nodes/MutationObserver-sanity.html" "/dom/nodes/MutationObserver-callback-arguments.html" "/dom/nodes/MutationObserver-takeRecords.html" "/dom/nodes/MutationObserver-disconnect.html" "/dom/nodes/mutationobservers.js" "/dom/nodes/MutationObserver-attributes.html" "/dom/nodes/MutationObserver-characterData.html"
git_wpt sparse-checkout add "/css/css-shadow/shadow-cascade-order-001.html"
git_wpt sparse-checkout add "/css/selectors/is-where-error-recovery.html"
git_wpt sparse-checkout add "/css/selectors/has-matches-to-uninserted-elements.html"
if [[ "$(git_wpt rev-parse HEAD 2>/dev/null || true)" == "$revision" ]]; then exit 0; fi
git_wpt fetch --depth 1 origin "$revision"
git_wpt checkout --detach "$revision"
