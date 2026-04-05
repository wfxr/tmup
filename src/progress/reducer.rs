use std::collections::HashMap;

use crate::progress::model::{OperationStage, PluginOutcome, PluginStage, PluginStageDetail};

/// Narrowed snapshot updates derived from public progress events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SnapshotUpdate {
    /// Operation-level stage transition.
    OperationStageChanged {
        /// New operation stage.
        stage: OperationStage,
    },
    /// Final operation failure state.
    OperationFailed {
        /// One-line failure summary.
        summary: String,
    },
    /// Plugin stage transition.
    PluginStageChanged {
        /// Canonical plugin id.
        id: String,
        /// New plugin stage.
        stage: PluginStage,
        /// Optional stage detail payload.
        detail: Option<PluginStageDetail>,
    },
    /// Final plugin completion outcome.
    PluginFinished {
        /// Canonical plugin id.
        id: String,
        /// Final outcome.
        outcome: PluginOutcome,
    },
    /// Final plugin failure state.
    PluginFailed {
        /// Canonical plugin id.
        id: String,
        /// Optional failure stage.
        stage: Option<PluginStage>,
        /// One-line failure summary.
        summary: String,
    },
}

/// Operation-level reducer snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct OperationSnapshot {
    /// Last known operation stage.
    pub(crate) stage: Option<OperationStage>,
    /// Terminal state once the operation has failed.
    pub(crate) terminal: Option<OperationTerminalState>,
}

/// Terminal operation state captured by the reducer snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OperationTerminalState {
    /// Operation failed with a one-line summary.
    Failed {
        /// One-line failure summary.
        summary: String,
    },
}

/// Plugin-level reducer snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginSnapshot {
    /// Canonical plugin id.
    pub(crate) id: String,
    /// Display label.
    pub(crate) label: String,
    /// Stable display slot.
    pub(crate) slot: usize,
    /// Current display state.
    pub(crate) state: PluginDisplayState,
}

/// Display state per plugin for reducer snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PluginDisplayState {
    /// Plugin has not emitted progress yet.
    Pending,
    /// Plugin is running in a stage.
    Running {
        /// Current stage.
        stage: PluginStage,
        /// Optional stage detail payload.
        detail: Option<PluginStageDetail>,
    },
    /// Plugin completed with a final outcome.
    Finished(PluginOutcome),
    /// Plugin failed with a final error summary.
    Failed {
        /// Optional stage at failure point.
        stage: Option<PluginStage>,
        /// One-line failure summary.
        summary: String,
    },
}

/// Full reducer snapshot for operation and plugins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgressSnapshot {
    /// Current operation state.
    pub(crate) operation: OperationSnapshot,
    /// Stable plugin snapshots in slot order.
    pub(crate) plugins: Vec<PluginSnapshot>,
    plugin_index: HashMap<String, usize>,
}

impl ProgressSnapshot {
    /// Construct a snapshot with fixed plugin slots from ordered `(id, label)` pairs.
    pub(crate) fn from_ordered_plugins(plugins: Vec<(String, String)>) -> Self {
        let mut entries = Vec::with_capacity(plugins.len());
        let mut plugin_index = HashMap::with_capacity(plugins.len());
        for (slot, (id, label)) in plugins.into_iter().enumerate() {
            plugin_index.insert(id.clone(), slot);
            entries.push(PluginSnapshot { id, label, slot, state: PluginDisplayState::Pending });
        }
        Self { operation: OperationSnapshot::default(), plugins: entries, plugin_index }
    }

    /// Ensure a plugin slot exists for `id`, adding a pending entry if absent.
    pub(crate) fn ensure_plugin(&mut self, id: &str, label: &str) {
        if self.plugin_index.contains_key(id) {
            return;
        }
        let slot = self.plugins.len();
        let id = id.to_string();
        self.plugin_index.insert(id.clone(), slot);
        self.plugins.push(PluginSnapshot {
            id,
            label: label.to_string(),
            slot,
            state: PluginDisplayState::Pending,
        });
    }

    /// Return plugin snapshot for `id`.
    pub(crate) fn plugin(&self, id: &str) -> Option<&PluginSnapshot> {
        self.plugin_index.get(id).and_then(|idx| self.plugins.get(*idx))
    }

    /// Construct a snapshot with fixed plugin slots for reducer tests.
    #[cfg(test)]
    pub(crate) fn new_for_tests<const N: usize>(plugins: [(&str, &str, usize); N]) -> Self {
        let mut entries = Vec::with_capacity(plugins.len());
        let mut index = HashMap::with_capacity(plugins.len());
        for (id, label, slot) in plugins {
            let id = id.to_string();
            index.insert(id.clone(), entries.len());
            entries.push(PluginSnapshot {
                id,
                label: label.to_string(),
                slot,
                state: PluginDisplayState::Pending,
            });
        }

        Self { operation: OperationSnapshot::default(), plugins: entries, plugin_index: index }
    }
}

/// Apply one structured progress event to a mutable snapshot.
pub(crate) fn apply_event(snapshot: &mut ProgressSnapshot, event: &SnapshotUpdate) {
    match event {
        SnapshotUpdate::OperationStageChanged { stage } => {
            snapshot.operation.stage = Some(*stage);
        }
        SnapshotUpdate::OperationFailed { summary } => {
            if snapshot.operation.terminal.is_none() {
                snapshot.operation.terminal =
                    Some(OperationTerminalState::Failed { summary: summary.clone() });
            }
        }
        SnapshotUpdate::PluginStageChanged { id, stage, detail } => {
            if let Some(plugin) = snapshot
                .plugin_index
                .get(id.as_str())
                .and_then(|idx| snapshot.plugins.get_mut(*idx))
            {
                if matches!(
                    plugin.state,
                    PluginDisplayState::Finished(_) | PluginDisplayState::Failed { .. }
                ) {
                    return;
                }
                plugin.state =
                    PluginDisplayState::Running { stage: *stage, detail: detail.clone() };
            }
        }
        SnapshotUpdate::PluginFinished { id, outcome } => {
            if let Some(plugin) = snapshot
                .plugin_index
                .get(id.as_str())
                .and_then(|idx| snapshot.plugins.get_mut(*idx))
            {
                if matches!(
                    plugin.state,
                    PluginDisplayState::Finished(_) | PluginDisplayState::Failed { .. }
                ) {
                    return;
                }
                plugin.state = PluginDisplayState::Finished(outcome.clone());
            }
        }
        SnapshotUpdate::PluginFailed { id, stage, summary } => {
            if let Some(plugin) = snapshot
                .plugin_index
                .get(id.as_str())
                .and_then(|idx| snapshot.plugins.get_mut(*idx))
            {
                if matches!(
                    plugin.state,
                    PluginDisplayState::Finished(_) | PluginDisplayState::Failed { .. }
                ) {
                    return;
                }
                plugin.state =
                    PluginDisplayState::Failed { stage: *stage, summary: summary.clone() };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ProgressSnapshot, SnapshotUpdate, apply_event};
    use crate::progress::model::{
        OperationStage, PluginOutcome, PluginStage, PluginStageDetail, SkipReason,
    };

    #[test]
    fn reducer_preserves_finished_plugin_slot() {
        let mut snapshot = ProgressSnapshot::new_for_tests([("github.com/acme/a", "plugin-a", 0)]);

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::OperationStageChanged { stage: OperationStage::Syncing },
        );
        assert_eq!(snapshot.operation.stage, Some(OperationStage::Syncing));
        assert_eq!(snapshot.operation.terminal, None);

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginStageChanged {
                id: "github.com/acme/a".to_string(),
                stage: PluginStage::Fetching,
                detail: Some(PluginStageDetail::CloneUrl(
                    "https://github.com/acme/a.git".to_string(),
                )),
            },
        );
        assert!(matches!(
            snapshot.plugins[0].state,
            super::PluginDisplayState::Running { stage: PluginStage::Fetching, .. }
        ));

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginStageChanged {
                id: "github.com/acme/a".to_string(),
                stage: PluginStage::Resolving,
                detail: None,
            },
        );
        assert!(matches!(
            snapshot.plugins[0].state,
            super::PluginDisplayState::Running { stage: PluginStage::Resolving, .. }
        ));

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginStageChanged {
                id: "github.com/acme/a".to_string(),
                stage: PluginStage::Applying,
                detail: Some(PluginStageDetail::BuildCommand("make build".to_string())),
            },
        );
        assert!(matches!(
            snapshot.plugins[0].state,
            super::PluginDisplayState::Running { stage: PluginStage::Applying, .. }
        ));

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginFinished {
                id: "github.com/acme/a".to_string(),
                outcome: PluginOutcome::Skipped {
                    reason: SkipReason::Other("not selected".to_string()),
                },
            },
        );
        assert_eq!(snapshot.plugins[0].slot, 0);
        assert!(matches!(
            snapshot.plugins[0].state,
            super::PluginDisplayState::Finished(PluginOutcome::Skipped { .. })
        ));

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginFailed {
                id: "github.com/acme/a".to_string(),
                stage: Some(PluginStage::Applying),
                summary: "build failed".to_string(),
            },
        );
        assert_eq!(snapshot.plugins[0].slot, 0);
        assert!(matches!(
            snapshot.plugins[0].state,
            super::PluginDisplayState::Finished(PluginOutcome::Skipped { .. })
        ));
    }

    #[test]
    fn reducer_handles_interleaved_plugins_and_direct_finish() {
        let mut snapshot = ProgressSnapshot::new_for_tests([
            ("github.com/acme/a", "plugin-a", 0),
            ("github.com/acme/b", "plugin-b", 1),
        ]);

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginStageChanged {
                id: "github.com/acme/a".to_string(),
                stage: PluginStage::Fetching,
                detail: None,
            },
        );
        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginFinished {
                id: "github.com/acme/b".to_string(),
                outcome: PluginOutcome::Installed { commit: "abc1234".to_string() },
            },
        );
        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginFailed {
                id: "github.com/acme/a".to_string(),
                stage: Some(PluginStage::Resolving),
                summary: "fetch failed".to_string(),
            },
        );

        assert!(matches!(
            snapshot.plugins[0].state,
            super::PluginDisplayState::Failed { stage: Some(PluginStage::Resolving), .. }
        ));
        assert!(matches!(
            snapshot.plugins[1].state,
            super::PluginDisplayState::Finished(PluginOutcome::Installed { .. })
        ));
        assert_eq!(snapshot.plugins[0].slot, 0);
        assert_eq!(snapshot.plugins[1].slot, 1);
    }

    #[test]
    fn reducer_ignores_unknown_plugin_ids() {
        let mut snapshot = ProgressSnapshot::new_for_tests([("github.com/acme/a", "plugin-a", 0)]);

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginFinished {
                id: "github.com/acme/missing".to_string(),
                outcome: PluginOutcome::CheckedUpToDate,
            },
        );
        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginFailed {
                id: "github.com/acme/missing".to_string(),
                stage: Some(PluginStage::Fetching),
                summary: "missing".to_string(),
            },
        );

        assert!(matches!(snapshot.plugins[0].state, super::PluginDisplayState::Pending));
        assert_eq!(snapshot.plugins.len(), 1);
    }

    #[test]
    fn reducer_does_not_reopen_plugin_after_terminal_state() {
        let mut snapshot = ProgressSnapshot::new_for_tests([("github.com/acme/a", "plugin-a", 0)]);

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginFinished {
                id: "github.com/acme/a".to_string(),
                outcome: PluginOutcome::Installed { commit: "abc1234".to_string() },
            },
        );
        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginStageChanged {
                id: "github.com/acme/a".to_string(),
                stage: PluginStage::Resolving,
                detail: None,
            },
        );

        assert!(matches!(
            snapshot.plugins[0].state,
            super::PluginDisplayState::Finished(PluginOutcome::Installed { .. })
        ));
    }

    #[test]
    fn reducer_keeps_first_terminal_state() {
        let mut snapshot = ProgressSnapshot::new_for_tests([("github.com/acme/a", "plugin-a", 0)]);

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginFailed {
                id: "github.com/acme/a".to_string(),
                stage: Some(PluginStage::Fetching),
                summary: "fetch failed".to_string(),
            },
        );
        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginFinished {
                id: "github.com/acme/a".to_string(),
                outcome: PluginOutcome::Installed { commit: "abc1234".to_string() },
            },
        );

        assert!(matches!(
            snapshot.plugins[0].state,
            super::PluginDisplayState::Failed { stage: Some(PluginStage::Fetching), .. }
        ));
    }

    #[test]
    fn reducer_does_not_reopen_failed_plugin_after_stage_change() {
        let mut snapshot = ProgressSnapshot::new_for_tests([("github.com/acme/a", "plugin-a", 0)]);

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginFailed {
                id: "github.com/acme/a".to_string(),
                stage: Some(PluginStage::Fetching),
                summary: "fetch failed".to_string(),
            },
        );
        apply_event(
            &mut snapshot,
            &SnapshotUpdate::PluginStageChanged {
                id: "github.com/acme/a".to_string(),
                stage: PluginStage::Resolving,
                detail: None,
            },
        );

        assert!(matches!(
            snapshot.plugins[0].state,
            super::PluginDisplayState::Failed { stage: Some(PluginStage::Fetching), .. }
        ));
    }

    #[test]
    fn reducer_records_operation_failure_terminal_state() {
        let mut snapshot = ProgressSnapshot::new_for_tests([("github.com/acme/a", "plugin-a", 0)]);

        apply_event(
            &mut snapshot,
            &SnapshotUpdate::OperationStageChanged { stage: OperationStage::Syncing },
        );
        apply_event(
            &mut snapshot,
            &SnapshotUpdate::OperationFailed { summary: "sync failed".to_string() },
        );

        assert_eq!(snapshot.operation.stage, Some(OperationStage::Syncing));
        assert!(matches!(
            snapshot.operation.terminal,
            Some(super::OperationTerminalState::Failed { ref summary })
                if summary == "sync failed"
        ));
    }
}
