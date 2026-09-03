#!/usr/bin/env bash
# typescript-language-server を reference/typescript-language-server からビルドする。
# 上流は pnpm (package.json の packageManager) を使う。pnpm と node は flake.nix のもの。
# tsserver (typescript) は flake.nix のものを --tsserver-path なしで PATH から探す。
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_clone typescript-language-server
(
  cd "$REFERENCE/typescript-language-server"
  pnpm install --frozen-lockfile --ignore-scripts
  pnpm run build
)
install_launcher typescript-language-server "node $REFERENCE/typescript-language-server/lib/cli.mjs"
echo "built: $("$BIN/typescript-language-server" --version)"
