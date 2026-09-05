//! サーバー状態プロトコルの `ServerState`（docs/spec/server-state.md 3 章・8.1）。
//!
//! `health` と `readiness` は独立の 2 軸。`message` は人間向けの補足であり
//! 機械判定に使ってはならない。ワイヤ形式は仕様が規範なので、本モジュールの
//! テストは serde の出力そのものを固定する。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 状態を問い合わせるリクエスト (仕様 4.1)。
///
/// LSP 本体に取り込まれるまで `experimental/` プレフィックスを使う
/// (仕様 4.3)。取り込み時に `workspace/` へ改名する。
pub const SERVER_STATE_METHOD: &str = "experimental/serverState";

/// 状態変化の通知 (仕様 4.2)。
pub const SERVER_STATE_CHANGED_METHOD: &str = "experimental/serverStateChanged";

/// サーバーが機能しているか (仕様 3 章)。
///
/// サーバーの死を表す値はない。プロセスの消失は接続の終了 (EOF) で伝える
/// (仕様 8.2 の 7、ADR 0009 決定 C-3)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ok,
    Warning,
    Error,
    /// health を観測する手段がない、またはまだ観測していない (最初の信号が
    /// 届く前)。観測者のみが送出する (仕様 8.1、8.2 の 2)。
    Unknown,
}

/// 要求に完全に答えられるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Readiness {
    Initializing,
    Indexing,
    Ready,
    /// readiness を観測する手段がない。観測者のみが送出する (仕様 8.1)。
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerState {
    pub health: Health,
    pub readiness: Readiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `ServerCapabilities.experimental.serverStateProvider` の値 (仕様 5 章、ADR 0016)。
///
/// 常にオブジェクト。`{}` は状態の通知だけを約束する。`coverage` と
/// `freshness` は `ready` が応答について何を意味するかを足し、値は
/// あるべき姿からの欠けを名指しする (真偽値ではない)。実装は自分が守れる
/// 保証だけを宣言する。守れない保証の宣言は仕様違反である (仕様 5.1)。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ServerStateProvider {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<Freshness>,
}

/// `ready` のとき、7.0 のメソッドの応答が基づく範囲と、件数の上限で
/// 結果を切るメソッド (メソッド名 → 上限)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub scope: CoverageScope,
    pub incomplete: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoverageScope {
    /// ワークスペース全体のインデックス。
    Workspace,
    /// クライアントが開いている文書だけ。
    OpenDocuments,
}

/// `ready` のとき織り込んでいる `workspace/didChangeWatchedFiles` の変更の
/// 種類。`textDocument/didChange` は常に織り込む。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freshness {
    #[serde(rename = "fileChanges")]
    pub file_changes: Vec<FileChangeType>,
}

/// LSP の `FileChangeType` の名前。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileChangeType {
    Created,
    Changed,
    Deleted,
}

/// 3 種類すべて (あるべき姿)。
pub const ALL_FILE_CHANGES: [FileChangeType; 3] = [
    FileChangeType::Created,
    FileChangeType::Changed,
    FileChangeType::Deleted,
];

impl ServerStateProvider {
    /// 状態の通知だけを約束する (保証なし)。
    pub fn notifications_only() -> Self {
        Self::default()
    }

    /// ワークスペース全体のインデックスに基づく `coverage` (上限で切る
    /// メソッドがあればその一覧) と、挙げた種類の変更を織り込む `freshness`。
    pub fn workspace(incomplete: &[(&str, u64)], file_changes: &[FileChangeType]) -> Self {
        ServerStateProvider {
            coverage: Some(Coverage {
                scope: CoverageScope::Workspace,
                incomplete: incomplete
                    .iter()
                    .map(|(method, limit)| (method.to_string(), *limit))
                    .collect(),
            }),
            freshness: Some(Freshness {
                file_changes: file_changes.to_vec(),
            }),
        }
    }
}

impl ServerState {
    /// `initialize` 直後の状態。まだ何も答えられない (仕様 7.1 の 1)。
    ///
    /// `health` は `unknown`。readiness と違い「initialize 直後」に対応する
    /// 既知の値がなく、最初の信号が届くまで `ok` を名乗るのは観測なしの
    /// 主張になる (ADR 0008 追補 E)。
    pub fn initializing() -> Self {
        ServerState {
            health: Health::Unknown,
            readiness: Readiness::Initializing,
            message: None,
        }
    }

    /// どちらの軸も観測できない状態。写像のない上流側はここから動かない
    /// (仕様 8.2 の 3)。`initializing` や `ok` から始めないのは、追跡して
    /// いないものを追跡しているように見せないため。
    pub fn unobserved() -> Self {
        ServerState {
            health: Health::Unknown,
            readiness: Readiness::Unknown,
            message: None,
        }
    }

    /// 通知を要する変化か。仕様 4.2 は「`health` または `readiness` が
    /// 変わるたびに送る」と定めており、`message` だけの変化は含まない。
    pub fn notifiable_change_from(&self, previous: &ServerState) -> bool {
        self.health != previous.health || self.readiness != previous.readiness
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_of(state: &ServerState) -> String {
        serde_json::to_string(state).unwrap()
    }

    #[test]
    fn serializes_to_the_shape_the_spec_defines() {
        let state = ServerState {
            health: Health::Ok,
            readiness: Readiness::Ready,
            message: Some("all good".to_string()),
        };
        assert_eq!(
            json_of(&state),
            r#"{"health":"ok","readiness":"ready","message":"all good"}"#
        );
    }

    #[test]
    fn omits_message_when_absent() {
        let state = ServerState {
            health: Health::Ok,
            readiness: Readiness::Indexing,
            message: None,
        };
        assert_eq!(json_of(&state), r#"{"health":"ok","readiness":"indexing"}"#);
    }

    #[test]
    fn uses_the_exact_health_strings_from_the_spec() {
        for (health, expected) in [
            (Health::Ok, "ok"),
            (Health::Warning, "warning"),
            (Health::Error, "error"),
            (Health::Unknown, "unknown"),
        ] {
            assert_eq!(
                serde_json::to_string(&health).unwrap(),
                format!("\"{expected}\"")
            );
        }
    }

    #[test]
    fn uses_the_exact_readiness_strings_from_the_spec() {
        for (readiness, expected) in [
            (Readiness::Initializing, "initializing"),
            (Readiness::Indexing, "indexing"),
            (Readiness::Ready, "ready"),
            (Readiness::Unknown, "unknown"),
        ] {
            assert_eq!(
                serde_json::to_string(&readiness).unwrap(),
                format!("\"{expected}\"")
            );
        }
    }

    #[test]
    fn dead_is_not_a_health_value() {
        // 仕様 3 章 (ADR 0009 決定 C-3): サーバーの死は値ではなく接続の終了で
        // 伝える。ワイヤに "dead" が現れたら、それは本仕様の値ではない。
        assert!(serde_json::from_str::<Health>("\"dead\"").is_err());
    }

    #[test]
    fn round_trips_through_json() {
        // 準拠テストスイート (偽クライアント側) が読み戻せる必要がある。
        let state = ServerState {
            health: Health::Warning,
            readiness: Readiness::Initializing,
            message: None,
        };
        let back: ServerState = serde_json::from_str(&json_of(&state)).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn initial_state_is_not_ready() {
        // 仕様 7.1 の 1: initialize 直後の readiness は ready ではない。
        let state = ServerState::initializing();
        assert_eq!(state.readiness, Readiness::Initializing);
        // health はまだ観測していない (ADR 0008 追補 E)。
        assert_eq!(state.health, Health::Unknown);
        assert_eq!(state.message, None);
    }

    #[test]
    fn a_change_on_either_axis_is_notifiable() {
        let base = ServerState::initializing();

        let readiness_moved = ServerState {
            readiness: Readiness::Ready,
            ..base.clone()
        };
        assert!(readiness_moved.notifiable_change_from(&base));

        let health_moved = ServerState {
            health: Health::Error,
            ..base.clone()
        };
        assert!(health_moved.notifiable_change_from(&base));
    }

    #[test]
    fn a_message_only_change_is_not_notifiable() {
        // 仕様 4.2 が挙げるのは health と readiness の 2 軸だけ。
        let base = ServerState::initializing();
        let same_axes = ServerState {
            message: Some("still loading crates".to_string()),
            ..base.clone()
        };
        assert!(!same_axes.notifiable_change_from(&base));
    }

    #[test]
    fn an_identical_state_is_not_notifiable() {
        let base = ServerState::initializing();
        assert!(!base.notifiable_change_from(&base));
    }

    #[test]
    fn the_unobserved_state_is_unknown_on_both_axes() {
        let state = ServerState::unobserved();
        assert_eq!(state.health, Health::Unknown);
        assert_eq!(state.readiness, Readiness::Unknown);
        assert_eq!(state.message, None);
    }

    #[test]
    fn notifications_only_serializes_as_an_empty_object() {
        // 仕様 5 章: {} は状態の通知だけを約束する (ADR 0016)。
        assert_eq!(
            serde_json::to_string(&ServerStateProvider::notifications_only()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn a_grade_omits_the_guarantees_it_does_not_claim() {
        // 守れない保証を宣言しないことが仕様 5.1 の要求。
        assert_eq!(
            serde_json::to_string(&ServerStateProvider::workspace(&[], &[])).unwrap(),
            r#"{"coverage":{"scope":"workspace","incomplete":{}},"freshness":{"fileChanges":[]}}"#
        );
    }

    #[test]
    fn a_grade_serializes_both_guarantees_when_claimed() {
        let both = ServerStateProvider::workspace(&[("workspace/symbol", 128)], &ALL_FILE_CHANGES);
        assert_eq!(
            serde_json::to_string(&both).unwrap(),
            r#"{"coverage":{"scope":"workspace","incomplete":{"workspace/symbol":128}},"freshness":{"fileChanges":["Created","Changed","Deleted"]}}"#
        );
    }
}
