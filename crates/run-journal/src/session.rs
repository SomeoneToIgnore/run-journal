use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use runvil::harness::{InferenceBroker, Packs, Record, RecordKind, RunId, Timer};
use runvil::plugin::PackRegistry;
use runvil::{
    Error, Harness, Host, Prompt, PromptFinder, RunControl, RunSetup, Timestamp, ToolInfo,
};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::watch;

use crate::answer;
use crate::broker::JournalBroker;
use crate::db::{Db, Stored};
use crate::journal::Launch;
use crate::normalize::{normalize, render};
use crate::packs;
use crate::recorder::JournalRecorder;
use crate::timer::JournalTimer;

#[derive(Clone)]
pub struct Session {
    shared: Arc<Shared>,
}

impl Session {
    #[must_use]
    pub fn launch(&self) -> &Launch {
        &self.shared.launch
    }

    pub fn host(
        &self,
        packs: PackRegistry,
        prompts: Arc<dyn PromptFinder>,
        timer: Arc<dyn Timer>,
    ) -> Result<Host, Error> {
        Ok(Host {
            packs: Packs::new(packs::wrap(&self.shared, packs)?),
            prompts,
            timer: Arc::new(JournalTimer::new(Arc::clone(&self.shared), timer)),
        })
    }

    #[must_use]
    pub fn setup(&self, broker: Arc<dyn InferenceBroker>) -> RunSetup {
        RunSetup {
            args: self.shared.launch.args.clone(),
            selected_model: self.shared.launch.model.clone(),
            ..RunSetup::new(
                Arc::new(JournalRecorder::new(Arc::clone(&self.shared))),
                Arc::new(JournalBroker::new(Arc::clone(&self.shared), broker)),
            )
        }
    }

    #[must_use]
    pub fn harness(&self, host: Arc<Host>, prompt: Arc<Prompt>, setup: RunSetup) -> Harness {
        let harness = Harness::new(host, prompt, setup);
        self.shared.state().control = Some(harness.control());
        harness
    }

    #[must_use]
    pub fn replaying(&self) -> bool {
        !self.shared.progress.borrow().settled()
    }

    pub async fn replayed(&self) -> bool {
        self.shared.replayed().await
    }

    pub(crate) fn new(db: Arc<AsyncMutex<Db>>, launch: Launch, resumed: Option<Resumed>) -> Self {
        let (run, started_at, old) = match resumed {
            Some(resumed) => (Some(resumed.run), Some(resumed.started_at), resumed.records),
            None => (None, None, Vec::new()),
        };
        let old = old
            .into_iter()
            .map(|stored| {
                let mut compared = stored.record.payload.clone();
                normalize(&mut compared);
                Old { stored, compared }
            })
            .collect::<Vec<_>>();
        let answers = old
            .iter()
            .enumerate()
            .filter(|(_, old)| old.stored.record.kind == RecordKind::Answer)
            .map(|(index, old)| (key(&old.stored.record), index))
            .collect::<HashMap<_, _>>();
        let total = old.len();
        let (progress, _) = watch::channel(Progress {
            cursor: 0,
            total,
            diverged: false,
        });
        Session {
            shared: Arc::new(Shared {
                db,
                launch,
                progress,
                state: Mutex::new(State {
                    run,
                    started_at,
                    next_seq: u64::try_from(total).unwrap_or(u64::MAX),
                    old,
                    answers,
                    cursor: 0,
                    rounds: HashMap::new(),
                    calls: Vec::new(),
                    timers: Vec::new(),
                    renders: HashMap::new(),
                    stash: HashMap::new(),
                    control: None,
                }),
            }),
        }
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Session")
            .field("launch", &self.shared.launch)
            .field("progress", &*self.shared.progress.borrow())
            .finish_non_exhaustive()
    }
}

pub(crate) struct Resumed {
    pub(crate) run: i64,
    pub(crate) started_at: Timestamp,
    pub(crate) records: Vec<Stored>,
}

pub(crate) struct Shared {
    pub(crate) db: Arc<AsyncMutex<Db>>,
    pub(crate) launch: Launch,
    progress: watch::Sender<Progress>,
    state: Mutex<State>,
}

pub(crate) type Key = (String, u32);

pub(crate) enum Claim {
    Stored {
        turn: usize,
        payload: Value,
        answer: Option<Value>,
    },
    Dropped {
        turn: usize,
    },
    Live {
        key: Option<Key>,
    },
}

pub(crate) enum Rendered {
    Stored { turn: usize, tools: Vec<ToolInfo> },
    Unreadable { turn: usize },
    Live,
}

pub(crate) enum Observed {
    Replayed,
    Diverged(String),
    Live { seq: u64, answer: Option<Value> },
}

impl Shared {
    pub(crate) fn resumed_run(&self) -> Option<RunId> {
        self.state().run.map(RunId::from_raw)
    }

    pub(crate) fn started_at(&self) -> Option<Timestamp> {
        self.state().started_at
    }

    pub(crate) fn begin(&self, run: RunId) {
        let mut state = self.state();
        state.run = Some(run.get());
        state.deliver();
    }

    pub(crate) fn ending(&self, result: &Result<String, Error>) -> Result<(), String> {
        let state = self.state();
        if state.cursor == state.old.len() {
            return Ok(());
        }
        self.progress
            .send_modify(|progress| progress.diverged = true);
        let ended = match result {
            Ok(_) => "a result".to_owned(),
            Err(error) => format!("the error: {}", error.located_message()),
        };
        Err(format!(
            "the run ended after replaying {} of the journal's {} records, so the journal keeps the run open; it ended with {ended}",
            state.cursor,
            state.old.len()
        ))
    }

    pub(crate) fn observe(&self, record: &Record) -> Observed {
        let mut state = self.state();
        let key = key(record);
        match record.kind {
            RecordKind::Effect => state.note(&key, &record.payload),
            RecordKind::Answer => state.settle(&key),
            RecordKind::Event => {}
        }
        if state.cursor == state.old.len() {
            let answer = (record.kind == RecordKind::Answer)
                .then(|| state.stash.remove(&key))
                .flatten()
                .filter(|answer| {
                    record.payload.get("Chat").is_some() || answer::agrees(answer, &record.payload)
                });
            let seq = state.next_seq;
            state.next_seq += 1;
            return Observed::Live { seq, answer };
        }
        let mut produced = record.payload.clone();
        normalize(&mut produced);
        let held = &state.old[state.cursor];
        let kept = &held.stored.record;
        if (kept.kind, &kept.task_id, kept.task_seq, &held.compared)
            != (record.kind, &record.task_id, record.task_seq, &produced)
        {
            self.progress
                .send_modify(|progress| progress.diverged = true);
            return Observed::Diverged(format!(
                "replay diverged at record {} of run {}: the journal holds {}, the run produced {}",
                state.cursor,
                state.run.unwrap_or_default(),
                render(kept),
                render(record)
            ));
        }
        state.cursor += 1;
        state.deliver();
        self.progress
            .send_modify(|progress| progress.cursor = state.cursor);
        Observed::Replayed
    }

    pub(crate) fn claim_round(&self, round: u64) -> Claim {
        let mut state = self.state();
        let key = state.rounds.remove(&round);
        state.claim(key)
    }

    pub(crate) fn claim_call(&self, call: &Value) -> Claim {
        let mut state = self.state();
        let key = state
            .calls
            .iter()
            .position(|(pending, _)| pending == call)
            .map(|at| state.calls.remove(at).1);
        state.claim(key)
    }

    pub(crate) fn claim_timer(&self, seconds: f64) -> Claim {
        let mut state = self.state();
        let key = state
            .timers
            .iter()
            .position(|(pending, _)| *pending == seconds.to_bits())
            .map(|at| state.timers.remove(at).1);
        state.claim(key)
    }

    pub(crate) fn claim_render(&self, entry: &str, pack: &Value) -> Rendered {
        let mut state = self.state();
        let state = &mut *state;
        let Some(pending) = state.renders.get_mut(entry) else {
            return Rendered::Live;
        };
        let call = pending.calls.entry(pack.to_string()).or_default();
        let nth = *call;
        *call += 1;
        let set = pending
            .offered
            .as_array()
            .and_then(|offered| offered.iter().find(|sets| sets.get("pack") == Some(pack)))
            .and_then(|sets| {
                ["static_set", "dynamic_set"]
                    .into_iter()
                    .filter(|set| {
                        sets.get(*set)
                            .and_then(Value::as_array)
                            .is_some_and(|tools| !tools.is_empty())
                    })
                    .nth(nth)
            });
        let Some(turn) = state.answers.get(&pending.key).copied() else {
            return Rendered::Live;
        };
        let tools = set
            .and_then(|set| {
                state.old[turn]
                    .stored
                    .record
                    .payload
                    .get("Render")?
                    .as_array()?
                    .iter()
                    .find(|sets| sets.get("pack") == Some(pack))?
                    .get(set)
                    .cloned()
            })
            .and_then(|tools| serde_json::from_value::<Vec<ToolInfo>>(tools).ok());
        match tools {
            Some(tools) => Rendered::Stored { turn, tools },
            None => Rendered::Unreadable { turn },
        }
    }

    pub(crate) fn stash(&self, key: Key, answer: Option<Value>) {
        if let Some(answer) = answer {
            self.state().stash.insert(key, answer);
        }
    }

    pub(crate) fn stop(&self) {
        if let Some(control) = &self.state().control {
            control.stop_round();
        }
    }

    pub(crate) async fn turn(&self, turn: usize) {
        let mut progress = self.progress.subscribe();
        let _ = progress.wait_for(|progress| progress.cursor >= turn).await;
    }

    pub(crate) async fn replayed(&self) -> bool {
        let mut progress = self.progress.subscribe();
        progress
            .wait_for(|progress| progress.settled())
            .await
            .is_ok_and(|progress| !progress.diverged)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Progress {
    cursor: usize,
    total: usize,
    diverged: bool,
}

impl Progress {
    fn settled(self) -> bool {
        self.diverged || self.cursor >= self.total
    }
}

struct State {
    run: Option<i64>,
    started_at: Option<Timestamp>,
    old: Vec<Old>,
    answers: HashMap<Key, usize>,
    cursor: usize,
    next_seq: u64,
    rounds: HashMap<u64, Key>,
    calls: Vec<(Value, Key)>,
    timers: Vec<(u64, Key)>,
    renders: HashMap<String, Render>,
    stash: HashMap<Key, Value>,
    control: Option<RunControl>,
}

struct Old {
    stored: Stored,
    compared: Value,
}

struct Render {
    key: Key,
    offered: Value,
    calls: HashMap<String, usize>,
}

impl State {
    fn note(&mut self, key: &Key, payload: &Value) {
        if let Some(round) = payload
            .get("Chat")
            .and_then(|chat| chat.get("round"))
            .and_then(Value::as_u64)
        {
            self.rounds.insert(round, key.clone());
        } else if let Some(call) = payload.get("ToolCall") {
            let call = Value::Array(
                ["tool", "args", "origin"]
                    .into_iter()
                    .map(|part| call.get(part).cloned().unwrap_or_default())
                    .collect::<Vec<_>>(),
            );
            self.calls.push((call, key.clone()));
        } else if let Some(seconds) = payload
            .get("Timer")
            .and_then(|timer| timer.get("seconds"))
            .and_then(Value::as_f64)
        {
            self.timers.push((seconds.to_bits(), key.clone()));
        } else if let Some(render) = payload.get("Render")
            && let Some(entry) = render.get("entry").and_then(Value::as_str)
        {
            self.renders.insert(
                entry.to_owned(),
                Render {
                    key: key.clone(),
                    offered: render.get("offered").cloned().unwrap_or_default(),
                    calls: HashMap::new(),
                },
            );
        }
    }

    fn settle(&mut self, key: &Key) {
        self.rounds.retain(|_, pending| pending != key);
        self.calls.retain(|(_, pending)| pending != key);
        self.timers.retain(|(_, pending)| pending != key);
        self.renders.retain(|_, pending| pending.key != *key);
    }

    fn claim(&self, key: Option<Key>) -> Claim {
        let Some(turn) = key.as_ref().and_then(|key| self.answers.get(key)).copied() else {
            return Claim::Live { key };
        };
        let stored = &self.old[turn].stored;
        if stored.record.payload == "Dropped" {
            return Claim::Dropped { turn };
        }
        Claim::Stored {
            turn,
            payload: stored.record.payload.clone(),
            answer: stored.answer.clone(),
        }
    }

    fn deliver(&self) {
        let Some(control) = &self.control else {
            return;
        };
        if self.cursor > 0 && self.old.get(self.cursor - 1).is_some_and(message) {
            return;
        }
        for old in self.old[self.cursor.min(self.old.len())..]
            .iter()
            .take_while(|old| message(old))
        {
            let text = old.stored.record.payload.get("text");
            if let Some(text) = text.and_then(Value::as_str) {
                let _ = control.send(text.to_owned());
            }
        }
    }
}

fn key(record: &Record) -> Key {
    (record.task_id.clone(), record.task_seq)
}

fn message(old: &Old) -> bool {
    let record = &old.stored.record;
    record.kind == RecordKind::Event
        && record.task_id == "0"
        && record.payload.get("kind").and_then(Value::as_str) == Some("inbox_message")
}
