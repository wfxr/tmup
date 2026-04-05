/// Structured operation-level progress stages used by the new reducer pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStage {
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
pub enum PluginStage {
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
pub enum PluginStageDetail {
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
pub enum TrackingSelector {
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
pub enum TrackingResolution {
    /// Default branch resolved to a concrete branch name.
    DefaultBranch {
        /// Concrete default branch name.
        branch: String,
    },
    /// Named branch resolution.
    Branch {
        /// Concrete branch name.
        branch: String,
    },
    /// Tag resolution.
    Tag {
        /// Concrete tag name.
        tag: String,
    },
    /// Commit resolution.
    Commit {
        /// Concrete commit hash.
        commit: String,
    },
}

impl PluginStageDetail {
    /// Build structured tracking-resolution detail from config and lock metadata.
    pub(crate) fn from_tracking(
        selector: &Tracking,
        resolved: &TrackingRecord,
        commit: &str,
    ) -> Self {
        let selector = match selector {
            Tracking::DefaultBranch => TrackingSelector::DefaultBranch,
            Tracking::Branch(branch) => TrackingSelector::Branch(branch.clone()),
            Tracking::Tag(tag) => TrackingSelector::Tag(tag.clone()),
            Tracking::Commit(commit) => TrackingSelector::Commit(commit.clone()),
        };
        let resolved = match resolved.kind.as_str() {
            "default-branch" => {
                TrackingResolution::DefaultBranch { branch: resolved.value.clone() }
            }
            "branch" => TrackingResolution::Branch { branch: resolved.value.clone() },
            "tag" => TrackingResolution::Tag { tag: resolved.value.clone() },
            "commit" => TrackingResolution::Commit { commit: resolved.value.clone() },
            _ => TrackingResolution::Commit { commit: resolved.value.clone() },
        };
        Self::TrackingResolution { selector, resolved, commit: short_hash(commit).to_string() }
    }
}

/// Final plugin outcomes emitted by the structured progress pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginOutcome {
    /// Plugin was newly installed at the given commit.
    Installed {
        /// Installed commit hash.
        commit: String,
    },
    /// Plugin was updated from one commit to another.
    Updated {
        /// Previously installed commit hash.
        from: String,
        /// Newly installed commit hash.
        to: String,
    },
    /// Sync phase published the specified commit.
    Synced {
        /// Published commit hash.
        commit: String,
    },
    /// Restore phase published the specified commit.
    Restored {
        /// Restored commit hash.
        commit: String,
    },
    /// Sync updated metadata without changing plugin contents.
    Reconciled,
    /// Plugin was checked and already up to date.
    CheckedUpToDate,
    /// Plugin was already at lock-pinned restore commit.
    AlreadyRestored,
    /// Plugin was skipped for a structured reason.
    Skipped {
        /// Structured reason for skip.
        reason: SkipReason,
    },
}

/// Structured skip reasons for plugin completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Plugin is pinned to a specific tag.
    PinnedTag {
        /// Pinned tag value.
        tag: String,
    },
    /// Plugin is pinned to a specific commit.
    PinnedCommit {
        /// Pinned commit hash.
        commit: String,
    },
    /// Plugin was skipped due to known failure marker at a commit.
    KnownFailure {
        /// Commit hash that matched a known-failure marker.
        commit: String,
    },
    /// Catch-all for other skip reasons.
    Other(String),
}

impl std::fmt::Display for OperationStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WaitingForLock => write!(f, "waiting"),
            Self::Syncing => write!(f, "syncing"),
            Self::ApplyingWrites => write!(f, "applying writes"),
            Self::LoadingTmux => write!(f, "loading tmux"),
        }
    }
}

impl std::fmt::Display for PluginStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cloning => write!(f, "cloning"),
            Self::Fetching => write!(f, "fetching"),
            Self::Resolving => write!(f, "resolving"),
            Self::CheckingOut => write!(f, "checking out"),
            Self::Applying => write!(f, "applying"),
        }
    }
}
use crate::lockfile::TrackingRecord;
use crate::model::Tracking;
use crate::short_hash;
