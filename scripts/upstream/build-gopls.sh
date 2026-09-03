#!/usr/bin/env bash
# gopls を reference/golang-tools からビルドする (go は flake.nix のもの)。
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_clone golang-tools
(cd "$REFERENCE/golang-tools/gopls" && go build -o "$BIN/gopls" .)
echo "built: $("$BIN/gopls" version)"
