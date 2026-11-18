use std::sync::Arc;
use std::time::Duration;

use runvil::harness::Timer;
use runvil::{BoxFuture, Timestamp};

use crate::session::Shared;

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
            let claim = shared.claim_timer(duration.as_secs_f64());
            let live = || inner.sleep(duration);
            shared.answer(claim, |_, _| {}, live, |()| None).await;
        })
    }

    fn now(&self) -> Timestamp {
        self.shared.started_at().unwrap_or_else(|| self.inner.now())
    }
}
