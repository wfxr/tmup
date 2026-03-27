//! Bounded concurrent prepare executor for remote plugin operations.

use std::future::Future;
use std::path::PathBuf;

use futures::stream::{self, StreamExt};

use crate::lockfile::TrackingRecord;

/// Immutable result of a successful prepare job.
pub struct PreparedPlugin {
    /// Declaration-order index for deterministic apply.
    pub index: usize,
    /// Canonical remote plugin identifier.
    pub id: String,
    /// Human-readable plugin name.
    pub name: String,
    /// Clone URL for the plugin repository.
    pub clone_url: String,
    /// Resolved commit to publish.
    pub resolved_commit: String,
    /// Tracking metadata resolved from the config or lock.
    pub tracking: Option<TrackingRecord>,
    /// Staging directory containing the checkout.
    pub staging_dir: PathBuf,
    /// Current disk HEAD commit, if the plugin is already installed.
    pub disk_commit: Option<String>,
    /// Config hash for the lock entry.
    pub config_hash: String,
}

/// Result of a failed prepare job.
pub struct PreparedFailure {
    /// Declaration-order index.
    pub index: usize,
    /// Canonical remote plugin identifier.
    pub id: String,
    /// Human-readable plugin name.
    pub name: String,
    /// The error that caused preparation to fail.
    pub error: anyhow::Error,
}

/// Outcome of a single prepare job.
pub enum PrepareOutcome {
    /// Plugin is ready for serial apply.
    Ready(PreparedPlugin),
    /// Plugin preparation failed.
    Failed(PreparedFailure),
}

/// Run up to `limit` futures concurrently, returning results in input order.
pub async fn run_bounded<F, T>(limit: usize, jobs: Vec<F>) -> Vec<T>
where
    F: Future<Output = T>,
{
    let limit = limit.max(1);
    let indexed = jobs.into_iter().enumerate().map(|(i, f)| async move { (i, f.await) });
    let mut results: Vec<_> = stream::iter(indexed).buffer_unordered(limit).collect().await;
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, v)| v).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn run_bounded_respects_concurrency_limit() {
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));

        let jobs: Vec<_> = (0..6)
            .map(|i| {
                let in_flight = in_flight.clone();
                let max_in_flight = max_in_flight.clone();
                async move {
                    let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_in_flight.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    i
                }
            })
            .collect();

        let results = run_bounded(2, jobs).await;

        assert_eq!(results, vec![0, 1, 2, 3, 4, 5]);
        assert!(max_in_flight.load(Ordering::SeqCst) <= 2);
        assert!(max_in_flight.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn run_bounded_serial_when_limit_one() {
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));

        let jobs: Vec<_> = (0..4)
            .map(|i| {
                let in_flight = in_flight.clone();
                let max_in_flight = max_in_flight.clone();
                async move {
                    let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_in_flight.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    i
                }
            })
            .collect();

        let results = run_bounded(1, jobs).await;

        assert_eq!(results, vec![0, 1, 2, 3]);
        assert_eq!(max_in_flight.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_bounded_preserves_input_order() {
        let jobs: Vec<_> = (0..5u64)
            .map(|i| async move {
                // Vary sleep so completion order differs from input order.
                tokio::time::sleep(Duration::from_millis((5 - i) * 10)).await;
                i as usize
            })
            .collect();

        let results = run_bounded(5, jobs).await;
        assert_eq!(results, vec![0, 1, 2, 3, 4]);
    }
}
