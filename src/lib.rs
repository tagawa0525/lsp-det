//! lsp-det: サーバー状態プロトコル (docs/spec/server-state.md) の参照実装となる
//! 透過プロキシ。上流側が言語サーバーを、下流側がクライアントを代行する。
//!
//! バイナリ本体は [`proxy::run`] を呼ぶだけの薄い層である。モジュールを
//! ライブラリとして公開しているのは、準拠テストスイート（`tests/`）が
//! フレーミングと `ServerState` の型定義を共有するため。スイートは
//! 実サーバーにも当てられる形で書く必要があり、型が二重定義になると
//! 「仕様に準拠しているか」を測る基準そのものがずれる。

pub mod adapter;
pub mod cli;
pub mod framing;
pub mod gate;
pub mod initialize;
pub mod peek;
pub mod process;
pub mod proxy;
pub mod state;
pub mod tracker;
