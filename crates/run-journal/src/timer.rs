use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use runvil::harness::Timer;
use runvil::{BoxFuture, Timestamp};

use crate::session::{Claim, Shared};

pub(crate) struct JournalTimer {
    shared: Arc<Shared>,
    inner: Arc<dyn Timer>,
}

impl JournalTimer {
    pub(crate) fn new(shared: Arc<Shared>, inner: Arc<dyn Timer>) -> Self {
        JournalTimer { shared, inner }
    }
}

impl Timer for JournalTimer {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        let shared = Arc::clone(&self.shared);
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            match shared.claim_timer(duration.as_secs_f64()) {
                Claim::Stored { turn, .. } => shared.turn(turn).await,
                Claim::Dropped { turn } => {
                    shared.turn(turn).await;
                    shared.stop();
                    pending::<()>().await;
                }
                Claim::Live { .. } => {
                    if !shared.replayed().await {
                        pending::<()>().await;
                    }
                    inner.sleep(duration).await;
                }
            }
        })
    }

    fn now(&self) -> Timestamp {
        self.shared.started_at().unwrap_or_else(|| self.inner.now())
    }
}
