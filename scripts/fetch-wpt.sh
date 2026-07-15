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
git_wpt sparse-checkout set "/resources/" "/dom/nodes/Element-childElement-null.html" "/dom/nodes/Element-childElementCount-nochild.html" "/dom/nodes/Node-isConnected.html" "/dom/nodes/Element-childElementCount.html" "/dom/nodes/Element-childElementCount-dynamic-add.html" "/dom/nodes/Element-childElementCount-dynamic-remove.html" "/dom/nodes/CharacterData-remove.html" "/dom/nodes/ChildNode-remove.js" "/dom/nodes/Element-remove.html" "/dom/nodes/DocumentType-remove.html" "/dom/nodes/Text-splitText.html" "/dom/nodes/CharacterData-data.html"
if [[ "$(git_wpt rev-parse HEAD 2>/dev/null || true)" == "$revision" ]]; then exit 0; fi
git_wpt fetch --depth 1 origin "$revision"
git_wpt checkout --detach "$revision"
