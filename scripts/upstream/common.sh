#!/usr/bin/env bash
# 上流をソース (reference/<repo>) からビルドし、target/upstream/bin に起動子を置く
# ための共通部分。各 build-*.sh が source する。
#
# 目的: 上流に出す変更 (serverInfo の追加、本プロトコルの実装、Serena の
# 読み取り側) をローカルで先に確かめる。reference/ の clone にパッチを当てて
# ビルドし、PATH の先頭に target/upstream/bin を置いて lsp-det の準拠テスト
# (cargo test --test conformance -- --ignored) を当てる。
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
REFERENCE="$ROOT/reference"
BIN="$ROOT/target/upstream/bin"
mkdir -p "$BIN"

# $1: 起動子の名前、$2: 実行するコマンド (文字列。"$@" が付く)
install_launcher() {
  local name="$1" command="$2"
  cat > "$BIN/$name" <<LAUNCHER
#!/usr/bin/env bash
exec $command "\$@"
LAUNCHER
  chmod +x "$BIN/$name"
  echo "installed: $BIN/$name -> $command"
}

require_clone() {
  local repo="$1"
  if [[ ! -d "$REFERENCE/$repo" ]]; then
    echo "reference/$repo がない。reference/README.md の手順で clone すること" >&2
    exit 1
  fi
}
