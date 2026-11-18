mod screen;

use std::env;
use std::error;
use std::fmt::Display;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use harness_gateway_client::{GatewayBroker, GatewayEndpoint, GatewaySearch, SecretString};
use plugin_web::SEARCH_PROVIDER;
use run_journal::{Journal, Launch, Session};
use runvil::effect::Round;
use runvil::harness::{InferenceBroker, Timer, insert_plugin_dir};
use runvil::model::{
    Completion, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
    make_completion_error, phrase,
};
use runvil::plugin::{HostServices, PackRegistry, ServiceKey, TOKIO_RUNTIME};
use runvil::vfs::{RealBackend, VfsRef, Writability};
use runvil::{BoxFuture, Error, ErrorCode, RunReport, RunSetup};
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::signal;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::screen::Screen;

type Failure = Box<dyn error::Error>;

type Gateway = (GatewayEndpoint, SecretString);

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
    let (vfs, workspace_roots) = files()?;
    let gateway = gateway()?;
    let session = open(journal, name).await?;
    let setup = session.setup(broker(gateway.clone()));
    let terminal = io::stdin().is_terminal() && io::stdout().is_terminal();
    let screen = Arc::new(Screen::new(
        Arc::clone(&setup.recorder),
        terminal,
        Box::new(print),
    ));
    let setup = RunSetup {
        recorder: screen.clone(),
        has_user: true,
        vfs,
        workspace_roots,
        ..setup
    };
    let harness = session.harness(
        packs(gateway)?,
        Arc::new(move |name: &str| find(&folder, name)),
        Arc::new(Clock),
        setup,
    )?;
    let control = harness.control();
    let mut lines = lines();
    let mut interrupts = interrupts();
    let mut waiting = screen.waiting();
    let mut settled = false;
    let mut caught_up = false;
    let mut open = true;
    let mut stopping = false;
    let report = harness.run();
    let replayed = session.replayed();
    tokio::pin!(report, replayed);
    loop {
        let ready = caught_up && open && *waiting.borrow();
        if ready && lines.is_empty() {
            screen.prompt();
        }
        tokio::select! {
            report = &mut report => {
                screen.interrupt();
                return finish(report);
            }
            whole = &mut replayed, if !settled => {
                settled = true;
                caught_up = whole;
            }
            line = lines.recv(), if ready => match line {
                Some(text) if text.trim().is_empty() => screen.skipped(),
                Some(text) => {
                    screen.sent();
                    let _ = control.send(text);
                }
                None => open = false,
            },
            Some(()) = interrupts.recv() => {
                if !screen.interrupt() && terminal {
                    eprintln!();
                }
                if !caught_up || stopping || *waiting.borrow() {
                    eprintln!("the run stays open; run the same command to resume it");
                    return Ok(ExitCode::from(130));
                }
                stopping = true;
                control.stop_round();
                eprintln!("stopping the reply; press Ctrl-C again to exit");
            }
            changed = waiting.changed() => changed?,
        }
        if *waiting.borrow() {
            stopping = false;
        }
        if caught_up && !open && *waiting.borrow() {
            screen.interrupt();
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

fn print(text: &str) {
    print!("{text}");
    let _ = io::stdout().flush();
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

fn interrupts() -> UnboundedReceiver<()> {
    let (sender, receiver) = unbounded_channel();
    tokio::spawn(async move { while signal::ctrl_c().await.is_ok() && sender.send(()).is_ok() {} });
    receiver
}

fn files() -> Result<(VfsRef, Vec<PathBuf>), Failure> {
    let Some(directory) = env::var_os("RUNVIL_FILES") else {
        return Ok((VfsRef::default(), Vec::new()));
    };
    let (vfs, root) = mount(Path::new(&directory))?;
    eprintln!("real files: {}, read-only", root.display());
    Ok((vfs, vec![root]))
}

fn mount(directory: &Path) -> Result<(VfsRef, PathBuf), Failure> {
    let refusal = |reason: &dyn Display| format!("RUNVIL_FILES {}: {reason}", directory.display());
    let root = fs::canonicalize(directory).map_err(|failure| refusal(&failure))?;
    let prefix = root
        .to_str()
        .filter(|prefix| prefix.starts_with('/') && *prefix != "/" && !prefix.contains('\\'))
        .ok_or_else(|| {
            refusal(&format_args!(
                "resolves to {}; a mount needs a UTF-8 path that starts with /, is not /, and holds no backslash",
                root.display()
            ))
        })?;
    let files = RealBackend::rooted(&root).map_err(|failure| refusal(&failure))?;
    let vfs = VfsRef::default().overlay(prefix, files.with_writability(Writability::ReadOnly));
    Ok((vfs, root))
}

fn gateway() -> Result<Option<Gateway>, Failure> {
    let Ok(url) = env::var("RUNVIL_GATEWAY_URL") else {
        eprintln!("RUNVIL_GATEWAY_URL is not set: running offline, every model round fails");
        return Ok(None);
    };
    let key = env::var("RUNVIL_GATEWAY_API_KEY")
        .map_err(|_| "RUNVIL_GATEWAY_URL is set, but RUNVIL_GATEWAY_API_KEY is not")?;
    Ok(Some((GatewayEndpoint::new(&url)?, SecretString::new(key)?)))
}

fn packs(gateway: Option<Gateway>) -> Result<PackRegistry, Failure> {
    let mut services = HostServices::new();
    services.provide(
        &ServiceKey::<Handle>::new(TOKIO_RUNTIME),
        Arc::new(Handle::current()),
    )?;
    if let Some((endpoint, key)) = gateway {
        services.provide(
            &SEARCH_PROVIDER,
            Arc::new(GatewaySearch::new(endpoint, key)),
        )?;
    }
    let mut registry = PackRegistry::new();
    plugin_web::install(&mut registry, Value::Null, &services)?;
    if let Ok(directory) = env::var("RUNVIL_PLUGINS") {
        let report = insert_plugin_dir(&mut registry, Path::new(&directory))?;
        for failure in report.failures() {
            eprintln!("a Plugin was not loaded: {}", failure.message);
        }
    }
    Ok(registry)
}

fn broker(gateway: Option<Gateway>) -> Arc<dyn InferenceBroker> {
    match gateway {
        Some((endpoint, key)) => Arc::new(GatewayBroker::new(endpoint, key)),
        None => Arc::new(Offline),
    }
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

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::process;
    use std::sync::atomic::{AtomicU32, Ordering};

    use runvil::vfs::Origin;

    use super::*;

    #[test]
    fn a_mounted_directory_is_read_only_at_its_own_path() {
        let directory = scratch("a_mounted_directory_is_read_only_at_its_own_path");
        let outside = scratch("a_mounted_directory_is_read_only_at_its_own_path");
        fs::write(directory.join("note.txt"), "ship on Friday").unwrap();
        fs::write(outside.join("secret.txt"), "the key").unwrap();
        symlink(outside.join("secret.txt"), directory.join("link.txt")).unwrap();

        let (vfs, root) = mount(&directory).unwrap();
        let files = vfs.acquire(Origin::new("test")).unwrap();
        let note = format!("{}/note.txt", root.display());
        let link = format!("{}/link.txt", root.display());
        let secret = fs::canonicalize(outside.join("secret.txt")).unwrap();

        assert_eq!(root, fs::canonicalize(&directory).unwrap());
        assert_eq!(files.read_string(&note), Ok("ship on Friday".to_owned()));
        assert_eq!(
            files
                .write(&note, b"ship today")
                .map_err(|refusal| refusal.tag()),
            Err("read_only")
        );
        assert_eq!(
            files.read_string(&link).map_err(|refusal| refusal.tag()),
            Err("outside_root")
        );
        assert_eq!(
            files
                .read_string(&secret.display().to_string())
                .map_err(|refusal| refusal.tag()),
            Err("not_found")
        );
        assert_eq!(files.write("/draft.txt", b"kept in memory"), Ok(()));
        assert_eq!(
            fs::read_to_string(directory.join("note.txt")).unwrap(),
            "ship on Friday"
        );

        fs::remove_dir_all(directory).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    fn scratch(test: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = env::temp_dir().join(format!("resumable-{test}-{}-{count}", process::id()));
        fs::create_dir_all(&directory).unwrap();
        directory
    }
}
