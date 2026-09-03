#!/usr/bin/env bash
# pyright を reference/pyright からビルドする。上流は pnpm のモノレポ
# (packages/pyright が言語サーバー)。pnpm と node は flake.nix のもの。
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_clone pyright
(
  cd "$REFERENCE/pyright"
  pnpm install --frozen-lockfile
  cd packages/pyright
  pnpm run build
)
install_launcher pyright-langserver "node $REFERENCE/pyright/packages/pyright/langserver.index.js"
echo "built: $(node "$REFERENCE/pyright/packages/pyright/index.js" --version)"
