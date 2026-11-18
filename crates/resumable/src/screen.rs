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

#[cfg(test)]
mod tests {
    use runvil::harness::MemoryRecorder;
    use runvil::{RunSeed, Timestamp};
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn a_journal_or_a_pipe_shows_each_block_under_its_role() {
        let desk = Desk::new(false).await;

        desk.screen.prompt();
        desk.event("inbox_waiting", "0", json!({})).await;
        desk.event(
            "inbox_message",
            "0",
            json!({ "text": "what is in the note?" }),
        )
        .await;
        desk.event("tool_result", "0", json!({ "alias": "read" }))
            .await;
        desk.event(
            "assistant_reply",
            "0",
            json!({ "text": "It says:\nship on Friday" }),
        )
        .await;
        desk.event(
            "inbox_message",
            "0.1",
            json!({ "text": "from another task" }),
        )
        .await;
        desk.event(
            "lua",
            "0",
            json!({ "message": "the reply ended early: canceled" }),
        )
        .await;

        assert_eq!(
            desk.shown(),
            concat!(
                "user> what is in the note?\n",
                "tool> read\n",
                "assistant> It says:\nship on Friday\n\n",
                "note> the reply ended early: canceled\n",
            )
        );
    }

    #[tokio::test]
    async fn a_terminal_labels_a_typed_line_once_and_an_early_line_at_its_turn() {
        let desk = Desk::new(true).await;
        let mut waiting = desk.screen.waiting();

        desk.event("inbox_waiting", "0", json!({})).await;
        desk.screen.prompt();
        desk.screen.prompt();
        desk.types("hey");
        desk.screen.sent();
        desk.event("inbox_message", "0", json!({ "text": "hey" }))
            .await;
        desk.types("early");
        desk.event("assistant_reply", "0", json!({ "text": "Hello." }))
            .await;
        desk.event("inbox_waiting", "0", json!({})).await;
        desk.screen.sent();
        desk.event("inbox_message", "0", json!({ "text": "early" }))
            .await;
        desk.event("inbox_waiting", "0", json!({})).await;
        desk.screen.prompt();
        waiting.mark_unchanged();
        desk.event("assistant_reply", "0.1", json!({ "text": "Aside." }))
            .await;
        let woken = waiting.has_changed();
        desk.screen.prompt();
        desk.types("");
        desk.screen.skipped();
        desk.screen.prompt();
        desk.screen.interrupt();

        assert_eq!(
            desk.shown(),
            concat!(
                "user> hey\n",
                "early\n",
                "assistant> Hello.\n\n",
                "user> early\n",
                "user> \n",
                "assistant> Aside.\n\n",
                "user> \n",
                "user> \n",
            )
        );
        assert_eq!(woken.ok(), Some(true));
    }

    struct Desk {
        screen: Screen,
        run: RunId,
        shown: Arc<Mutex<String>>,
    }

    impl Desk {
        async fn new(terminal: bool) -> Self {
            let shown = Arc::new(Mutex::new(String::new()));
            let sink = Arc::clone(&shown);
            let screen = Screen::new(
                Arc::new(MemoryRecorder::new()),
                terminal,
                Box::new(move |text| sink.lock().unwrap().push_str(text)),
            );
            let meta = RunMeta {
                seed: RunSeed::from_bytes([0; 32]),
                flags: 0,
                started_at: Timestamp::from_unix_millis(0),
            };
            let run = screen.begin_run(meta).await.unwrap();
            Desk { screen, run, shown }
        }

        async fn event(&self, kind: &str, task: &str, mut payload: Value) {
            payload["kind"] = Value::from(kind);
            payload["provenance"] = json!({ "task": task, "seq": 0 });
            let record = Record {
                task_id: task.to_owned(),
                task_seq: 0,
                kind: RecordKind::Event,
                effect_id: None,
                payload,
            };
            self.screen.append(self.run, record).await.unwrap();
        }

        fn types(&self, line: &str) {
            let mut shown = self.shown.lock().unwrap();
            shown.push_str(line);
            shown.push('\n');
        }

        fn shown(&self) -> String {
            self.shown.lock().unwrap().clone()
        }
    }
}
