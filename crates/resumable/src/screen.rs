use std::sync::Arc;

use run_journal::Session;
use runvil::harness::{Record, RecordKind, RunId, RunMeta, RunRecorder};
use runvil::{BoxFuture, Error};
use serde_json::Value;
use tokio::sync::watch;

pub(crate) struct Screen {
    session: Session,
    inner: Arc<dyn RunRecorder>,
    echo: bool,
    waiting: watch::Sender<bool>,
}

impl Screen {
    pub(crate) fn new(session: Session, inner: Arc<dyn RunRecorder>, echo: bool) -> Self {
        Screen {
            inner,
            session,
            echo,
            waiting: watch::Sender::new(false),
        }
    }

    pub(crate) fn waiting(&self) -> watch::Receiver<bool> {
        self.waiting.subscribe()
    }

    pub(crate) fn sent(&self) {
        self.waiting.send_replace(false);
    }

    fn show(&self, event: &Value, replaying: bool) {
        let main = event.pointer("/provenance/task").and_then(Value::as_str) == Some("0");
        let text = event
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event.get("kind").and_then(Value::as_str) {
            Some("inbox_message") if main => {
                self.waiting.send_replace(false);
                if replaying || self.echo {
                    println!("> {text}");
                }
            }
            Some("assistant_reply") => println!("{text}\n"),
            Some("tool_result") => {
                let alias = event
                    .get("alias")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                println!("[{alias}]");
            }
            Some("inbox_waiting") if main => {
                self.waiting.send_replace(true);
            }
            _ => {}
        }
    }
}

impl RunRecorder for Screen {
    fn begin_run(&self, meta: RunMeta) -> BoxFuture<'_, Result<RunId, Error>> {
        self.inner.begin_run(meta)
    }

    fn append(&self, run: RunId, record: Record) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let replaying = self.session.replaying();
            let event = (record.kind == RecordKind::Event).then(|| record.payload.clone());
            self.inner.append(run, record).await?;
            if let Some(event) = event {
                self.show(&event, replaying);
            }
            Ok(())
        })
    }

    fn end_run(
        &self,
        run: RunId,
        result: Result<String, Error>,
    ) -> BoxFuture<'_, Result<(), Error>> {
        self.inner.end_run(run, result)
    }
}
