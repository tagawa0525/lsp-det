//! 拡張 S の `ServerState` (docs/spec/extension-s-server-state.md 3 章)。
//!
//! `health` と `readiness` は独立の 2 軸。`message` は人間向けの補足であり
//! 機械判定に使ってはならない。ワイヤ形式は仕様が規範なので、本モジュールの
//! テストは serde の出力そのものを固定する。

use serde::{Deserialize, Serialize};

/// サーバーが機能しているか。`dead` は中継層だけが送出できる (仕様 6.1)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ok,
    Warning,
    Error,
    Dead,
}

/// 要求に完全に答えられるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Readiness {
    Initializing,
    Indexing,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerState {
    pub health: Health,
    pub readiness: Readiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ServerState {
    /// `initialize` 直後の状態。まだ何も答えられない (仕様 7.1 の 1)。
    pub fn initializing() -> Self {
        todo!("M2: 初期状態を返す")
    }

    /// 通知を要する変化か。仕様 4.2 は「`health` または `readiness` が
    /// 変わるたびに送る」と定めており、`message` だけの変化は含まない。
    pub fn notifiable_change_from(&self, previous: &ServerState) -> bool {
        todo!("M2: 2 軸の変化だけを見る")
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
            (Health::Dead, "dead"),
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
        ] {
            assert_eq!(
                serde_json::to_string(&readiness).unwrap(),
                format!("\"{expected}\"")
            );
        }
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
        assert_eq!(state.health, Health::Ok);
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
}
