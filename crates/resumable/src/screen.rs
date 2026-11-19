use std::sync::{Arc, Mutex, PoisonError};

use runvil::harness::{Record, RecordKind, RunId, RunMeta, RunRecorder};
use runvil::{BoxFuture, Error};
use serde_json::Value;
use tokio::sync::watch;

pub(crate) type Sink = Box<dyn Fn(&str) + Send + Sync>;

pub(crate) struct Screen {
    inner: Arc<dyn RunRecorder>,
    terminal: bool,
    sink: Sink,
    waiting: watch::Sender<bool>,
    prompt: Mutex<Prompt>,
}

impl Screen {
    pub(crate) fn new(inner: Arc<dyn RunRecorder>, terminal: bool, sink: Sink) -> Self {
        Screen {
            inner,
            terminal,
            sink,
            waiting: watch::Sender::new(false),
            prompt: Mutex::new(Prompt::Closed),
        }
    }

    pub(crate) fn waiting(&self) -> watch::Receiver<bool> {
        self.waiting.subscribe()
    }

    pub(crate) fn prompt(&self) {
        if self.terminal && self.turn(Prompt::Closed, Prompt::Open) {
            (self.sink)(USER);
        }
    }

    pub(crate) fn sent(&self) {
        self.waiting.send_replace(false);
        self.turn(Prompt::Open, Prompt::Typed);
    }

    pub(crate) fn skipped(&self) {
        self.turn(Prompt::Open, Prompt::Closed);
    }

    pub(crate) fn interrupt(&self) -> bool {
        let open = self.turn(Prompt::Open, Prompt::Closed);
        if open {
            (self.sink)("\n");
        }
        open
    }

    fn show(&self, event: &Value) {
        let main = event.pointer("/provenance/task").and_then(Value::as_str) == Some("0");
        let text = event
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event.get("kind").and_then(Value::as_str) {
            Some("inbox_message") if main => {
                self.waiting.send_replace(false);
                if !self.turn(Prompt::Typed, Prompt::Closed) {
                    self.block(&format!("{USER}{text}\n"));
                }
            }
            Some("assistant_reply") => self.block(&format!("assistant> {text}\n\n")),
            Some("tool_result") => {
                let alias = event
                    .get("alias")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.block(&format!("tool> {alias}\n"));
            }
            Some("lua") => {
                let message = event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.block(&format!("note> {message}\n"));
            }
            Some("inbox_waiting") if main => {
                self.waiting.send_replace(true);
            }
            _ => {}
        }
    }

    fn block(&self, text: &str) {
        let interrupted = self.interrupt();
        (self.sink)(text);
        if interrupted {
            self.waiting.send_modify(|_| {});
        }
    }

    fn turn(&self, from: Prompt, to: Prompt) -> bool {
        let mut prompt = self.prompt.lock().unwrap_or_else(PoisonError::into_inner);
        let turned = *prompt == from;
        if turned {
            *prompt = to;
        }
        turned
    }
}

impl RunRecorder for Screen {
    fn begin_run(&self, meta: RunMeta) -> BoxFuture<'_, Result<RunId, Error>> {
        self.inner.begin_run(meta)
    }

    fn append(&self, run: RunId, record: Record) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let event = (record.kind == RecordKind::Event).then(|| record.payload.clone());
            self.inner.append(run, record).await?;
            if let Some(event) = event {
                self.show(&event);
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

const USER: &str = "user> ";

#[derive(Clone, Copy, PartialEq)]
enum Prompt {
    Closed,
    Open,
    Typed,
}
