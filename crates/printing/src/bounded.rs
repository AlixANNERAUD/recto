//! Blocking work on documents users send, with bounded concurrency and time, for
//! [`PageCounter`](crate::count::PageCounter) and
//! [`RasterConverter`](crate::raster::RasterConverter).
//!
//! The work runs on blocking threads, which cannot be cancelled: work that times out keeps its
//! place until it ends, so the number of threads and the memory they hold stay bounded whatever
//! is sent.

use std::{sync::Arc, time::Duration};

use tokio::{sync::Semaphore, time::Instant};

/// At most a number of pieces of work at once, each within a time limit.
#[derive(Clone, Debug)]
pub(crate) struct Bounded {
    pub(crate) permits: Arc<Semaphore>,
    timeout: Duration,
}

/// Why the work gave no result.
pub(crate) enum Late {
    /// Other work held every place until the time limit.
    Busy,
    /// The work did not end in the time left, or its thread panicked.
    TimedOut,
}

impl Bounded {
    /// At most `concurrency` at once, each within `timeout`.
    pub(crate) fn new(concurrency: usize, timeout: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(concurrency.max(1))),
            timeout,
        }
    }

    /// Runs `work` on a blocking thread once a place is free. The time limit counts from the
    /// call, waiting for a place included, so a caller never waits more than it in all; `work`
    /// gets that deadline, to stop by itself when it can.
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(std::time::Instant) -> T + Send + 'static,
    ) -> Result<T, Late> {
        let deadline = Instant::now() + self.timeout;
        let permit = tokio::time::timeout_at(deadline, self.permits.clone().acquire_owned())
            .await
            .map_err(|_| Late::Busy)?
            .map_err(|_| Late::Busy)?;
        let task = tokio::task::spawn_blocking(move || {
            let done = work(deadline.into_std());
            drop(permit);
            done
        });
        match tokio::time::timeout_at(deadline, task).await {
            Ok(Ok(done)) => Ok(done),
            Ok(Err(_)) | Err(_) => Err(Late::TimedOut),
        }
    }
}
