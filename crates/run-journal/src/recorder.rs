use std::sync::Arc;

use runvil::harness::{Record, RunId, RunMeta, RunRecorder, make_recorder_error};
use runvil::{BoxFuture, Error};

use crate::session::{Observed, Shared};

pub(crate) struct JournalRecorder {
    shared: Arc<Shared>,
}

impl JournalRecorder {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        JournalRecorder { shared }
    }
}

impl RunRecorder for JournalRecorder {
    fn begin_run(&self, meta: RunMeta) -> BoxFuture<'_, Result<RunId, Error>> {
        Box::pin(async move {
            let run = if let Some(run) = self.shared.resumed_run() {
                run
            } else {
                let mut db = self.shared.db.lock().await;
                db.begin(&self.shared.launch, meta.seed, meta.started_at)
                    .await
                    .map(RunId::from_raw)
                    .map_err(make_recorder_error)?
            };
            self.shared.begin(run);
            Ok(run)
        })
    }

    fn append(&self, run: RunId, record: Record) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            match self.shared.observe(&record) {
                Observed::Replayed => Ok(()),
                Observed::Diverged(difference) => Err(make_recorder_error(difference)),
                Observed::Live { seq, answer } => {
                    let mut db = self.shared.db.lock().await;
                    db.append(run.get(), seq, &record, answer.as_ref())
                        .await
                        .map_err(make_recorder_error)
                }
            }
        })
    }

    fn end_run(
        &self,
        run: RunId,
        result: Result<String, Error>,
    ) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            self.shared.ending(&result).map_err(make_recorder_error)?;
            let (status, result) = match result {
                Ok(text) => ("completed", text),
                Err(error) if error.is_canceled() => ("canceled", error.located_message()),
                Err(error) => ("failed", error.located_message()),
            };
            let mut db = self.shared.db.lock().await;
            db.end(run.get(), status, &result)
                .await
                .map_err(make_recorder_error)
        })
    }
}
