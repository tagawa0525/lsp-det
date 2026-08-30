//! lsp-det: LSP 拡張「Server State（拡張 S）」の参照実装となる透過プロキシ。
//!
//! バイナリ本体は [`proxy::run`] を呼ぶだけの薄い層である。モジュールを
//! ライブラリとして公開しているのは、準拠テストスイート（`tests/`）が
//! フレーミングと `ServerState` の型定義を共有するため。スイートは
//! 実サーバーにも当てられる形で書く必要があり、型が二重定義になると
//! 「仕様に準拠しているか」を測る基準そのものがずれる。

pub mod adapter;
pub mod cli;
pub mod framing;
pub mod initialize;
pub mod peek;
pub mod process;
pub mod proxy;
pub mod state;
pub mod tracker;
