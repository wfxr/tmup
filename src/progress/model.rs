#![allow(dead_code)]

/// Structured operation-level progress stages used by the new reducer pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationStage {
    /// Waiting to acquire the global operation lock.
    WaitingForLock,
    /// Reconciling remote plugin metadata and lock snapshots.
    Syncing,
    /// Applying resolved plugin contents to target directories.
    ApplyingWrites,
    /// Applying the load plan to the current tmux session.
    LoadingTmux,
}

/// Structured plugin-level progress stages used by the new reducer pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginStage {
    /// Clone the repository for first-time installation.
    Cloning,
    /// Fetch updates for an existing repository.
    Fetching,
    /// Resolve tracking selectors to concrete commits.
    Resolving,
    /// Check out the chosen commit in staging.
    CheckingOut,
    /// Build and publish staged content.
    Applying,
}

/// Stage-specific detail payload for structured plugin progress updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PluginStageDetail {
    /// Remote clone URL associated with clone/fetch stages.
    CloneUrl(String),
    /// Structured selector-to-resolution mapping for resolving stages.
    TrackingResolution {
        /// Selector declared in user configuration.
        selector: TrackingSelector,
        /// Concrete value resolved from the selector.
        resolved: TrackingResolution,
        /// Resolved target commit hash.
        commit: String,
    },
    /// Build command used during apply/publish.
    BuildCommand(String),
}

/// Tracking selector kind declared in configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrackingSelector {
    /// Follow the repository default branch.
    DefaultBranch,
    /// Follow a named branch.
    Branch(String),
    /// Track a named tag.
    Tag(String),
    /// Pin directly to a commit hash.
    Commit(String),
}

/// Concrete tracking target resolved from a selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrackingResolution {
    /// Default branch resolved to a concrete branch name.
    DefaultBranch { branch: String },
    /// Named branch resolution.
    Branch { branch: String },
    /// Tag resolution.
    Tag { tag: String },
    /// Commit resolution.
    Commit { commit: String },
}

/// Final plugin outcomes emitted by the structured progress pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PluginOutcome {
    /// Plugin was newly installed at the given commit.
    Installed { commit: String },
    /// Plugin was updated from one commit to another.
    Updated { from: String, to: String },
    /// Sync phase published the specified commit.
    Synced { commit: String },
    /// Restore phase published the specified commit.
    Restored { commit: String },
    /// Sync updated metadata without changing plugin contents.
    Reconciled,
    /// Plugin was checked and already up to date.
    CheckedUpToDate,
    /// Plugin was already at lock-pinned restore commit.
    AlreadyRestored,
    /// Plugin was skipped for a structured reason.
    Skipped { reason: SkipReason },
}

/// Structured skip reasons for plugin completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SkipReason {
    /// Plugin is pinned to a specific tag.
    PinnedTag { tag: String },
    /// Plugin is pinned to a specific commit.
    PinnedCommit { commit: String },
    /// Plugin was skipped due to known failure marker at a commit.
    KnownFailure { commit: String },
    /// Catch-all for other skip reasons.
    Other(String),
}
