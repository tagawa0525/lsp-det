//! `ServerState` of the server state protocol (docs/spec/server-state.md chapter 3 and 8.1).
//!
//! `health` and `readiness` are two independent axes. `message` is a supplement for humans
//! and must not be used for machine judgment. The wire format is normative in the spec, so
//! the tests in this module pin down the serde output itself.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The request that asks for the state (spec 4.1).
///
/// The `experimental/` prefix is used until it is taken into LSP itself (spec 4.3). It is
/// renamed to `workspace/` when taken in.
pub const SERVER_STATE_METHOD: &str = "experimental/serverState";

/// The notification of a state change (spec 4.2).
pub const SERVER_STATE_CHANGED_METHOD: &str = "experimental/serverStateChanged";

/// Whether the server is functioning (spec chapter 3).
///
/// There is no value for the server's death. The disappearance of the process is conveyed by
/// the end of the connection (EOF) (spec 8.2 item 7, ADR 0009 decision C-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ok,
    Warning,
    Error,
    /// There is no means to observe health, or it has not been observed yet (before the first
    /// signal arrives). Emitted only by observers (spec 8.1, 8.2 item 2).
    Unknown,
}

/// Whether requests can be answered completely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Readiness {
    Initializing,
    Indexing,
    Ready,
    /// There is no means to observe readiness. Emitted only by observers (spec 8.1).
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerState {
    pub health: Health,
    pub readiness: Readiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The value of `ServerCapabilities.experimental.serverStateProvider` (spec chapter 5,
/// ADR 0016).
///
/// Always an object. `{}` promises only the state notifications. `coverage` and `freshness`
/// add what `ready` means for responses, and their values name what is missing from the ideal
/// (they are not booleans). An implementation declares only the guarantees it can keep.
/// Declaring a guarantee it cannot keep is a spec violation (spec 5.1).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ServerStateProvider {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<Freshness>,
}

/// When `ready`, the scope the responses of the 7.0 methods are based on, and the methods
/// that cap their results at a count (method name -> cap).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub scope: CoverageScope,
    pub incomplete: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoverageScope {
    /// The index of the whole workspace.
    Workspace,
    /// Only the documents the client has open.
    OpenDocuments,
}

/// The kinds of `workspace/didChangeWatchedFiles` changes incorporated when `ready`.
/// `textDocument/didChange` is always incorporated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freshness {
    #[serde(rename = "fileChanges")]
    pub file_changes: Vec<FileChangeType>,
}

/// The names of LSP's `FileChangeType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileChangeType {
    Created,
    Changed,
    Deleted,
}

/// All 3 kinds (the ideal).
pub const ALL_FILE_CHANGES: [FileChangeType; 3] = [
    FileChangeType::Created,
    FileChangeType::Changed,
    FileChangeType::Deleted,
];

impl ServerStateProvider {
    /// Promises only the state notifications (no guarantees).
    pub fn notifications_only() -> Self {
        Self::default()
    }

    /// `coverage` based on the index of the whole workspace (with the list of methods that
    /// cap, if any) and `freshness` that incorporates the listed kinds of changes.
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
    /// The state right after `initialize`. Nothing can be answered yet (spec 7.1 item 1).
    ///
    /// `health` is `unknown`. Unlike readiness, there is no known value that corresponds to
    /// "right after initialize", and claiming `ok` until the first signal arrives would be an
    /// assertion without observation (ADR 0008 addendum E).
    pub fn initializing() -> Self {
        ServerState {
            health: Health::Unknown,
            readiness: Readiness::Initializing,
            message: None,
        }
    }

    /// The state where neither axis can be observed. An upstream side without a mapping never
    /// moves from here (spec 8.2 item 3). It does not start from `initializing` or `ok` so as
    /// not to appear to track what is not being tracked.
    pub fn unobserved() -> Self {
        ServerState {
            health: Health::Unknown,
            readiness: Readiness::Unknown,
            message: None,
        }
    }

    /// Whether this is a change that requires a notification. Spec 4.2 says "send every time
    /// `health` or `readiness` changes", which excludes a change of `message` alone.
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
        // Spec chapter 3 (ADR 0009 decision C-3): the server's death is conveyed by the end of
        // the connection, not by a value. If "dead" appears on the wire, it is not a value of
        // this spec.
        assert!(serde_json::from_str::<Health>("\"dead\"").is_err());
    }

    #[test]
    fn round_trips_through_json() {
        // The conformance test suite (the fake client side) must be able to read it back.
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
        // Spec 7.1 item 1: readiness right after initialize is not ready.
        let state = ServerState::initializing();
        assert_eq!(state.readiness, Readiness::Initializing);
        // health has not been observed yet (ADR 0008 addendum E).
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
        // Spec 4.2 lists only the two axes health and readiness.
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
        // Spec chapter 5: {} promises only the state notifications (ADR 0016).
        assert_eq!(
            serde_json::to_string(&ServerStateProvider::notifications_only()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn a_declaration_omits_the_guarantees_it_does_not_claim() {
        // Not declaring a guarantee it cannot keep is what spec 5.1 requires.
        assert_eq!(
            serde_json::to_string(&ServerStateProvider::workspace(&[], &[])).unwrap(),
            r#"{"coverage":{"scope":"workspace","incomplete":{}},"freshness":{"fileChanges":[]}}"#
        );
    }

    #[test]
    fn a_declaration_serializes_both_guarantees_when_claimed() {
        let both = ServerStateProvider::workspace(&[("workspace/symbol", 128)], &ALL_FILE_CHANGES);
        assert_eq!(
            serde_json::to_string(&both).unwrap(),
            r#"{"coverage":{"scope":"workspace","incomplete":{"workspace/symbol":128}},"freshness":{"fileChanges":["Created","Changed","Deleted"]}}"#
        );
    }
}
