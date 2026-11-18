mod screen;

use std::env;
use std::error;
use std::fs;
use std::io::{self, BufRead, IsTerminal};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use harness_gateway_client::{GatewayBroker, GatewayEndpoint, SecretString};
use run_journal::{Journal, Launch, Session};
use runvil::effect::Round;
use runvil::harness::{InferenceBroker, Timer, insert_plugin_dir};
use runvil::model::{
    Completion, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
    make_completion_error, phrase,
};
use runvil::plugin::PackRegistry;
use runvil::{BoxFuture, Error, ErrorCode, RunReport, RunSetup};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::screen::Screen;

type Failure = Box<dyn error::Error>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let (Some(journal), Some(prompt), None) =
        (arguments.next(), arguments.next(), arguments.next())
    else {
        eprintln!("usage: resumable <journal.db> <prompt.md>");
        return ExitCode::from(2);
    };
    match run(Path::new(&journal), Path::new(&prompt)).await {
        Ok(code) => code,
        Err(failure) => {
            eprintln!("error: {failure}");
            ExitCode::FAILURE
        }
    }
}

async fn run(journal: &Path, prompt: &Path) -> Result<ExitCode, Failure> {
    let name = prompt
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("the prompt path names no file")?;
    let folder = prompt.parent().map(Path::to_owned).unwrap_or_default();
    let session = open(journal, name).await?;
    let host = session.host(
        packs()?,
        Arc::new(move |name: &str| find(&folder, name)),
        Arc::new(Clock),
    )?;
    let host = Arc::new(host);
    let parsed = host.parse(&session.launch().prompt)?;
    let setup = session.setup(broker()?);
    let screen = Arc::new(Screen::new(
        session.clone(),
        Arc::clone(&setup.recorder),
        !io::stdin().is_terminal(),
    ));
    let setup = RunSetup {
        recorder: screen.clone(),
        has_user: true,
        ..setup
    };
    let harness = session.harness(host, parsed, setup);
    let control = harness.control();
    let mut lines = lines();
    let mut waiting = screen.waiting();
    let mut settled = false;
    let mut caught_up = false;
    let mut open = true;
    let report = harness.run();
    let replayed = session.replayed();
    tokio::pin!(report, replayed);
    loop {
        tokio::select! {
            report = &mut report => return finish(report),
            whole = &mut replayed, if !settled => {
                settled = true;
                caught_up = whole;
            }
            line = lines.recv(), if caught_up && open && *waiting.borrow() => match line {
                Some(text) if text.trim().is_empty() => {}
                Some(text) => {
                    screen.sent();
                    let _ = control.send(text);
                }
                None => open = false,
            },
            changed = waiting.changed() => changed?,
        }
        if caught_up && !open && *waiting.borrow() {
            eprintln!("the run waits for input; run the same command to resume it");
            return Ok(ExitCode::SUCCESS);
        }
    }
}

async fn open(journal: &Path, name: &str) -> Result<Session, Failure> {
    let journal = Journal::open(journal).await?;
    match journal.unfinished().await? {
        Some((run, launch)) if launch.prompt == name => {
            eprintln!("resuming run {run}");
            Ok(journal.resume(run).await?)
        }
        Some((run, launch)) => Err(Failure::from(format!(
            "the journal's unfinished run {run} is of prompt {}, not {name}; pass that prompt or another journal",
            launch.prompt
        ))),
        None => Ok(journal.start(Launch {
            prompt: name.to_owned(),
            args: "{}".to_owned(),
            model: env::var("RUNNER_MODEL").ok(),
        })),
    }
}

fn finish(report: Result<RunReport, Error>) -> Result<ExitCode, Failure> {
    match report?.result {
        Ok(text) => {
            println!("{text}");
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!("the run failed: {}", error.located_message());
            Ok(ExitCode::FAILURE)
        }
    }
}

fn lines() -> UnboundedReceiver<String> {
    let (sender, receiver) = unbounded_channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    receiver
}

fn packs() -> Result<PackRegistry, Failure> {
    let mut registry = PackRegistry::new();
    if let Ok(directory) = env::var("RUNVIL_PLUGINS") {
        let report = insert_plugin_dir(&mut registry, Path::new(&directory))?;
        for failure in report.failures() {
            eprintln!("a Plugin was not loaded: {}", failure.message);
        }
    }
    Ok(registry)
}

fn broker() -> Result<Arc<dyn InferenceBroker>, Failure> {
    let Ok(url) = env::var("RUNVIL_GATEWAY_URL") else {
        eprintln!("RUNVIL_GATEWAY_URL is not set: running offline, every model round fails");
        return Ok(Arc::new(Offline));
    };
    let key = env::var("RUNVIL_GATEWAY_API_KEY")
        .map_err(|_| "RUNVIL_GATEWAY_URL is set, but RUNVIL_GATEWAY_API_KEY is not")?;
    let broker = GatewayBroker::new(GatewayEndpoint::new(&url)?, SecretString::new(key)?);
    Ok(Arc::new(broker))
}

fn find(folder: &Path, name: &str) -> Result<String, Error> {
    let path = folder.join(name);
    fs::read_to_string(&path).map_err(|failure| {
        let code = if failure.kind() == io::ErrorKind::NotFound {
            ErrorCode::NotFound
        } else {
            ErrorCode::Io
        };
        Error::new(code, format!("{}: {failure}", path.display()), code.prose())
    })
}

struct Offline;

impl InferenceBroker for Offline {
    fn models(&self) -> BoxFuture<'static, Result<ModelCatalog, Error>> {
        Box::pin(async { Ok(ModelCatalog::empty()) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _round: Round,
    ) -> BoxFuture<'static, Result<Box<Completion>, Error>> {
        let code = ErrorCode::Unavailable;
        Box::pin(async move { Err(make_completion_error(code, phrase(code))) })
    }
}

struct Clock;

impl Timer for Clock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}
