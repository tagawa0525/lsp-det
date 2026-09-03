#!/usr/bin/env bash
# rust-analyzer を reference/rust-analyzer からビルドする。上流の rust-version
# (Cargo.toml) が flake.nix の rustc より新しいことがあるので、rustup の stable
# で組む (rustup がなければ flake の cargo を試す)。
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
require_clone rust-analyzer
(
  cd "$REFERENCE/rust-analyzer"
  if command -v rustup >/dev/null; then
    rustup run stable cargo build --release -p rust-analyzer
  else
    cargo build --release -p rust-analyzer
  fi
)
install_launcher rust-analyzer "$REFERENCE/rust-analyzer/target/release/rust-analyzer"
echo "built: $("$BIN/rust-analyzer" --version)"
