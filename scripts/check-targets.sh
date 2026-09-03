#!/usr/bin/env bash
# macOS と Windows のターゲットで cargo check を通す（ADR 0012 決定 D）。
# Linux の開発環境から、他 OS でコンパイルが通るかを push の前に確かめる。
# 挙動の検証は CI の各 OS のランナーが行う（.github/workflows/ci.yml）。
#
# rustup の stable を使う（nix の cargo は他ターゲットの std を持たない）。
# 生成物は target/cross に置き、通常のビルドと混ざらないようにする。
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/cross}"

for target in aarch64-apple-darwin x86_64-pc-windows-msvc; do
  if ! rustup target list --installed --toolchain stable | grep -qx "$target"; then
    rustup target add "$target" --toolchain stable
  fi
  echo "== cargo check --target $target"
  rustup run stable cargo check --target "$target" --tests --examples
done
