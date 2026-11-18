use std::collections::VecDeque;
use std::env;
use std::fs;
use std::future::{Future, pending};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use run_journal::{Journal, Launch, Session};
use runvil::effect::Round;
use runvil::harness::{InferenceBroker, Packs, Record, RunId, RunMeta, RunRecorder, Timer};
use runvil::metrics::{CallMetrics, ClientTiming};
use runvil::model::{
    Completion, CompletionOptions, CompletionResult, Message, ModelBinding, ModelCatalog, ModelId,
    ModelInfo, ThinkingMode, ToolCall, ToolSchema, make_completion_error, phrase,
};
use runvil::packs::PackId;
use runvil::plugin::{Pack, PackInfo, PackRegistry, ToolContext};
use runvil::{
    BoxFuture, Error, ErrorCode, Harness, Host, PromptFinder, RunControl, RunReport, RunSetup,
    Timestamp, ToolId, ToolInfo, ToolOutput,
};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub(crate) const DESK: &str = concat!(
    "---\nname: desk\ndescription: Talks with the operator over the desk's notes\nrunvil: 0\n",
    "uses:\n  - user\n  - acme/desk\n",
    "---\n\n## Conversation\n\n```lua\n",
    "tools.offer('desk')\n",
    "while true do\n",
    "  messages:user(tasks.receive())\n",
    "  pcall(models.loop, models.get('default'))\n",
    "end\n",
    "```\n",
);

pub(crate) const BARE: &str = concat!(
    "---\nname: desk\ndescription: Talks with the operator over the desk's notes\nrunvil: 0\n",
    "uses:\n  - user\n  - acme/desk\n",
    "---\n\n## Conversation\n\n```lua\n",
    "while true do\n",
    "  messages:user(tasks.receive())\n",
    "  pcall(models.loop, models.get('default'))\n",
    "end\n",
    "```\n",
);

pub(crate) const ONCE: &str = concat!(
    "---\nname: desk\ndescription: Repeats the operator's one message\nrunvil: 0\n",
    "uses:\n  - user\n",
    "---\n\n## Only\n\n```lua\n",
    "return 'You said: ' .. tasks.receive()\n",
    "```\n",
);

pub(crate) const TASKS: &str = concat!(
    "---\nname: desk\ndescription: Works the desk with two tasks, then talks\nrunvil: 0\n",
    "uses:\n  - user\n  - acme/desk\n",
    "---\n\n## Main\n\n```lua\n",
    "tools.offer('desk')\n",
    "store.write('seen.md', tostring(sys.when))\n",
    "local quick = tasks.spawn('../Quick')\n",
    "tasks.join_any({ quick }, { timeout = 30 })\n",
    "local slow = tasks.spawn('../Slow')\n",
    "local none = tasks.join_any({ slow }, { timeout = 0.05 })\n",
    "store.write('seen.md', tostring(none))\n",
    "while true do\n",
    "  messages:user(tasks.receive())\n",
    "  pcall(models.loop, models.get('default'))\n",
    "end\n",
    "```\n\n## Quick\n\n```lua\n",
    "return tools.call('desk/read')\n",
    "```\n\n## Slow\n\n```lua\n",
    "return models.infer('Take your time.')\n",
    "```\n",
);

pub(crate) type Shown = (String, u32, &'static str, Value);

#[derive(Clone, Copy)]
pub(crate) enum Reply {
    Text(&'static str),
    Held(&'static str),
    Read,
    ReadTwice,
    Fail,
    Stuck,
}

#[derive(Clone, Copy)]
pub(crate) enum Step {
    Wait(usize),
    Event(&'static str, usize),
    Send(&'static str),
    Started(usize),
    Release,
    Stop,
}

pub(crate) struct Outcome {
    pub(crate) records: Vec<Shown>,
    pub(crate) requests: Vec<Vec<String>>,
    pub(crate) reads: usize,
}

pub(crate) struct Phase {
    control: RunControl,
    session: Option<Session>,
    task: JoinHandle<Result<RunReport, Error>>,
    tee: Arc<Tee>,
    script: Arc<Script>,
    reads: Arc<AtomicUsize>,
}

impl Phase {
    pub(crate) async fn journaled(path: &Path, source: &'static str, replies: &[Reply]) -> Phase {
        Phase::over(path, source, Script::new(replies, true)).await
    }

    pub(crate) async fn unlisted(path: &Path, source: &'static str) -> Phase {
        Phase::over(path, source, Script::new(&[], false)).await
    }

    pub(crate) fn uninterrupted(source: &'static str, replies: &[Reply]) -> Phase {
        let script = Arc::new(Script::new(replies, true));
        let reads = Arc::new(AtomicUsize::new(0));
        let tee = Arc::new(Tee::new(Arc::new(Null)));
        let host = Arc::new(Host {
            packs: Packs::new(desk(&reads)),
            prompts: finder(source),
            timer: Arc::new(Clock { now: FIRST_START }),
        });
        let prompt = host.parse(PROMPT).unwrap();
        let setup = RunSetup {
            has_user: true,
            ..RunSetup::new(tee.clone(), script.clone())
        };
        let harness = Harness::new(host, prompt, setup);
        Phase {
            control: harness.control(),
            session: None,
            task: tokio::spawn(harness.run()),
            tee,
            script,
            reads,
        }
    }

    pub(crate) async fn replayed(&self) {
        if let Some(session) = &self.session {
            assert!(within(session.replayed()).await);
        }
    }

    pub(crate) fn requests(&self) -> usize {
        self.script.requests.lock().unwrap().len()
    }

    pub(crate) fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    pub(crate) async fn play(&self, steps: &[Step]) {
        let mut seen = self.tee.seen.subscribe();
        let mut started = self.script.started.subscribe();
        for step in steps {
            match *step {
                Step::Wait(count) => {
                    within(seen.wait_for(|_| self.tee.count("inbox_waiting") >= count))
                        .await
                        .unwrap();
                }
                Step::Event(kind, count) => {
                    within(seen.wait_for(|_| self.tee.count(kind) >= count))
                        .await
                        .unwrap();
                }
                Step::Send(text) => assert!(self.control.send(text.to_owned())),
                Step::Started(count) => {
                    within(started.wait_for(|started| *started >= count))
                        .await
                        .unwrap();
                }
                Step::Release => {
                    self.script.released.send_replace(true);
                }
                Step::Stop => self.control.stop_round(),
            }
        }
    }

    pub(crate) async fn cut(self) -> Outcome {
        self.task.abort();
        let _ = self.task.await;
        outcome(&self.tee, &self.script, &self.reads)
    }

    pub(crate) async fn ended(self) -> (Result<RunReport, Error>, Outcome) {
        let report = within(self.task).await.unwrap();
        (report, outcome(&self.tee, &self.script, &self.reads))
    }

    async fn over(path: &Path, source: &'static str, script: Script) -> Phase {
        let journal = Journal::open(path).await.unwrap();
        let (session, now) = if let Some((run, _)) = journal.unfinished().await.unwrap() {
            (journal.resume(run).await.unwrap(), LATER_START)
        } else {
            let launch = Launch {
                prompt: PROMPT.to_owned(),
                args: "{}".to_owned(),
                model: None,
            };
            (journal.start(launch), FIRST_START)
        };
        let script = Arc::new(script);
        let reads = Arc::new(AtomicUsize::new(0));
        let host = session
            .host(desk(&reads), finder(source), Arc::new(Clock { now }))
            .unwrap();
        let host = Arc::new(host);
        let prompt = host.parse(PROMPT).unwrap();
        let setup = session.setup(script.clone());
        let tee = Arc::new(Tee::new(Arc::clone(&setup.recorder)));
        let setup = RunSetup {
            recorder: tee.clone(),
            has_user: true,
            ..setup
        };
        let harness = session.harness(host, prompt, setup);
        Phase {
            control: harness.control(),
            session: Some(session),
            task: tokio::spawn(harness.run()),
            tee,
            script,
            reads,
        }
    }
}

pub(crate) async fn uninterrupted(
    source: &'static str,
    replies: &[Reply],
    steps: &[Step],
) -> Outcome {
    let phase = Phase::uninterrupted(source, replies);
    phase.play(steps).await;
    phase.cut().await
}

pub(crate) async fn journal(path: &Path) -> Vec<Shown> {
    let journal = Journal::open(path).await.unwrap();
    let (run, _) = journal.unfinished().await.unwrap().unwrap();
    journal
        .records(run)
        .await
        .unwrap()
        .iter()
        .map(shown)
        .collect::<Vec<_>>()
}

pub(crate) async fn unfinished(path: &Path) -> Option<i64> {
    let journal = Journal::open(path).await.unwrap();
    journal
        .unfinished()
        .await
        .unwrap()
        .map(|(run, _)| run.get())
}

pub(crate) fn journal_path(test: &str) -> PathBuf {
    let folder = env::temp_dir().join(format!(
        "run-journal-{test}-{}-{}",
        process::id(),
        FOLDERS.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(&folder).unwrap();
    folder.join("journal.db")
}

pub(crate) fn remove(path: &Path) {
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

const PROMPT: &str = "desk.md";

const MODEL: &str = "stub-model";

const AWKWARD: f64 = 3.907_800_000_000_000_4;

const GUARD: &str = "untrusted_input_";

const FIRST_START: Timestamp = Timestamp::from_unix_millis(1_700_000_000_000);

const LATER_START: Timestamp = Timestamp::from_unix_millis(1_700_086_400_000);

const DESK_PACK: PackInfo = PackInfo::new("acme/desk").description("Reads the notes on the desk");

static FOLDERS: AtomicUsize = AtomicUsize::new(0);

struct Script {
    catalog: Option<ModelCatalog>,
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<Vec<String>>>,
    started: watch::Sender<usize>,
    released: watch::Sender<bool>,
}

impl Script {
    fn new(replies: &[Reply], listed: bool) -> Script {
        let window = NonZeroU32::new(131_072).unwrap();
        let model = ModelInfo::new(
            ModelId::gateway(MODEL).unwrap(),
            "stub",
            window,
            ThinkingMode::Never,
        );
        Script {
            catalog: listed.then(|| ModelCatalog::new([model]).unwrap()),
            replies: Mutex::new(replies.iter().copied().collect::<VecDeque<_>>()),
            requests: Mutex::new(Vec::new()),
            started: watch::Sender::new(0),
            released: watch::Sender::new(false),
        }
    }
}

impl InferenceBroker for Script {
    fn models(&self) -> BoxFuture<'static, Result<ModelCatalog, Error>> {
        let catalog = self.catalog.clone();
        Box::pin(async move { catalog.ok_or_else(unavailable) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _round: Round,
    ) -> BoxFuture<'static, Result<Box<Completion>, Error>> {
        let request = messages
            .iter()
            .map(|message| scrub(&format!("{message:?}")))
            .collect::<Vec<_>>();
        self.requests.lock().unwrap().push(request);
        let reply = self.replies.lock().unwrap().pop_front();
        self.started.send_modify(|started| *started += 1);
        let tool = tools
            .first()
            .map(|tool| tool.name().to_owned())
            .unwrap_or_default();
        let mut released = self.released.subscribe();
        Box::pin(async move {
            if let Some(Reply::Held(_)) = reply {
                let _ = released.wait_for(|released| *released).await;
            }
            match reply {
                Some(Reply::Text(text) | Reply::Held(text)) => {
                    let metrics = CallMetrics::new().with_client(Some(ClientTiming::new(AWKWARD)));
                    Completion::from_result(CompletionResult::Text(text.to_owned()), MODEL).map(
                        |completion| {
                            Box::new(
                                completion
                                    .with_finish_reason("stop")
                                    .with_reasoning_content("Thinking it over.")
                                    .with_metrics(metrics),
                            )
                        },
                    )
                }
                Some(Reply::Read) => calls(&tool, &["call-1"]),
                Some(Reply::ReadTwice) => calls(&tool, &["call-1", "call-2"]),
                Some(Reply::Stuck) => pending().await,
                Some(Reply::Fail) | None => Err(unavailable()),
            }
        })
    }
}

struct Desk {
    read: ToolInfo,
    reads: Arc<AtomicUsize>,
}

impl Pack for Desk {
    fn tools(&self) -> Vec<ToolInfo> {
        Vec::from([self.read.clone()])
    }

    fn call<'a>(
        &'a self,
        _cx: ToolContext<'a>,
        _args: Value,
    ) -> BoxFuture<'a, Result<ToolOutput, Error>> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            Ok(ToolOutput::untrusted(format!(
                "Ship on Friday, read {read}."
            )))
        })
    }
}

struct Tee {
    inner: Arc<dyn RunRecorder>,
    records: Mutex<Vec<Record>>,
    seen: watch::Sender<usize>,
}

impl Tee {
    fn new(inner: Arc<dyn RunRecorder>) -> Tee {
        Tee {
            inner,
            records: Mutex::new(Vec::new()),
            seen: watch::Sender::new(0),
        }
    }

    fn count(&self, kind: &str) -> usize {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.payload["kind"] == kind)
            .count()
    }
}

impl RunRecorder for Tee {
    fn begin_run(&self, meta: RunMeta) -> BoxFuture<'_, Result<RunId, Error>> {
        self.inner.begin_run(meta)
    }

    fn append(&self, run: RunId, record: Record) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            self.inner.append(run, record.clone()).await?;
            self.records.lock().unwrap().push(record);
            self.seen.send_modify(|seen| *seen += 1);
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

struct Null;

impl RunRecorder for Null {
    fn begin_run(&self, _meta: RunMeta) -> BoxFuture<'_, Result<RunId, Error>> {
        Box::pin(async { Ok(RunId::from_raw(1)) })
    }

    fn append(&self, _run: RunId, _record: Record) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }

    fn end_run(
        &self,
        _run: RunId,
        _result: Result<String, Error>,
    ) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}

struct Clock {
    now: Timestamp,
}

impl Timer for Clock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration))
    }

    fn now(&self) -> Timestamp {
        self.now
    }
}

fn finder(source: &'static str) -> Arc<dyn PromptFinder> {
    Arc::new(move |name: &str| {
        if name == PROMPT {
            return Ok(source.to_owned());
        }
        let code = ErrorCode::NotFound;
        Err(Error::new(
            code,
            format!("no prompt named {name}"),
            code.prose(),
        ))
    })
}

fn desk(reads: &Arc<AtomicUsize>) -> PackRegistry {
    let reads = Arc::clone(reads);
    let mut registry = PackRegistry::new();
    registry
        .insert(DESK_PACK, move |id: &PackId| {
            let read = ToolInfo::new(
                ToolId::parse(&format!("{id}/read"))?,
                "Reads the notes on the desk.",
                json!({ "type": "object", "properties": {} }),
            );
            let desk = Desk {
                read,
                reads: Arc::clone(&reads),
            };
            Ok(Arc::new(desk) as Arc<dyn Pack>)
        })
        .unwrap();
    registry
}

fn calls(tool: &str, ids: &[&str]) -> Result<Box<Completion>, Error> {
    let calls = ids
        .iter()
        .map(|id| ToolCall::from_parts(*id, tool, json!({})))
        .collect::<Result<Vec<_>, _>>()?;
    Completion::from_result(CompletionResult::ToolCalls(calls), MODEL).map(Box::new)
}

fn unavailable() -> Error {
    let code = ErrorCode::Unavailable;
    make_completion_error(code, phrase(code))
}

fn outcome(tee: &Tee, script: &Script, reads: &AtomicUsize) -> Outcome {
    Outcome {
        records: tee
            .records
            .lock()
            .unwrap()
            .iter()
            .map(shown)
            .collect::<Vec<_>>(),
        requests: script.requests.lock().unwrap().clone(),
        reads: reads.load(Ordering::SeqCst),
    }
}

fn shown(record: &Record) -> Shown {
    (
        record.task_id.clone(),
        record.task_seq,
        record.kind.as_str(),
        serde_json::from_str::<Value>(&scrub(&record.payload.to_string())).unwrap(),
    )
}

fn scrub(text: &str) -> String {
    let mut pieces = text.split(GUARD);
    let mut scrubbed = pieces.next().unwrap_or_default().to_owned();
    for piece in pieces {
        scrubbed.push_str(GUARD);
        let nonce = piece
            .chars()
            .take(32)
            .take_while(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase())
            .count();
        if nonce == 32 {
            scrubbed.push_str("<nonce>");
            scrubbed.push_str(&piece[32..]);
        } else {
            scrubbed.push_str(piece);
        }
    }
    scrubbed
}

async fn within<Output>(future: impl Future<Output = Output>) -> Output {
    tokio::time::timeout(Duration::from_secs(20), future)
        .await
        .unwrap()
}
