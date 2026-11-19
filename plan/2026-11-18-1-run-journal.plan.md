---
name: Run Journal
overview: A crate over the `runvil` facade keeps every record and every full answer of a run in a Turso file, and a new process resumes that run by replaying the answers in their recorded order, with a command-line Host that proves it against a real model.
todos:
  - id: step1-journal
    content: "Step 1: the journal file, its schema, and the recorder that writes it"
    status: completed
  - id: step2-answers
    content: "Step 2: store the full answer of each model round and tool call, and rebuild it"
    status: completed
  - id: step3-replay
    content: "Step 3: replay in recorded order through the broker, the Packs, the timer, and the run's control"
    status: completed
  - id: step4-tests
    content: "Step 4: compare each resumed run with the same script run without a stop"
    status: completed
  - id: step5-command-line
    content: "Step 5: the `resumable` command-line Host and `run.sh`"
    status: completed
  - id: step6-seed
    content: "Step 6: pass the recorded seed back through the Harness, then compare records exactly"
    status: pending
  - id: step7-files
    content: "Step 7: keep a run's files across exits"
    status: pending
  - id: step8-catalog
    content: "Step 8: replay the model catalog the run started with"
    status: pending
  - id: step9-workshop
    content: "Step 9: resume Workshop conversations from the journal"
    status: pending
  - id: step10-home
    content: "Step 10: choose the crate's repository and name"
    status: pending
  - id: step11-tools
    content: "Step 11: real files and web pages for the command-line Host"
    status: completed
  - id: step12-stop
    content: "Step 12: stop a reply from the command line, and keep the run when a reply ends early"
    status: completed
isProject: false
---

# A run resumes from its journal

Today a run lives only as long as its process.
When the process ends, the Engine's state ends with it, the conversation included.
This plan keeps a journal of the run in a file and resumes the run from it in a new process.
Nothing is restored: the run starts again from its first line, and every answer it had comes back from the journal instead of from a model or a tool.
The Engine is deterministic for the same inputs, so the run reaches the state it had.

Steps 1 to 5, 11, and 12 are built and pass their checks.
Steps 6 to 10 are open.

## Notes

- Repository: `/Users/someonetoignore/work/cppalliance/run-journal`, a Rust workspace of two crates.
  Every path in this plan is relative to its root, unless the text names another repository.
- Crates: `crates/run-journal` is the library, and `crates/resumable` is the command-line Host.
- The Harness: `runvil`, `harness-gateway-client`, and `plugin-web` come from `cppalliance/runvil-harness` at `db4cf1f4` (`Cargo.toml` lines 13 to 15).
  Harness paths in this plan are relative to that repository at that commit.
- Toolchain: Rust 1.99 (`rust-toolchain.toml`), as the Harness pins.
- Build: `cargo build --workspace`.
- Tests: `cargo test --workspace`.
- Lint: `cargo clippy --workspace --all-targets`.
  The lint table follows the Harness's (`Cargo.toml` lines 22 to 42).
  It leaves out `missing_docs`, the three rustdoc lints, the three `disallowed_*` lints, and the three lints about unsafe code beside `unsafe_code`.
  It allows `missing_errors_doc` and `missing_panics_doc`, because the code has no doc comments.
- Format: `cargo fmt --all --check`.
- Run: `./run.sh [prompt.md] [journal.db]`.
  With no arguments it runs `prompts/agent.md` over `agent.db` in the root, with `~/Downloads` mounted read-only.
  `RUNVIL_FILES=<directory>` before the command mounts that directory instead, and an empty `RUNVIL_FILES` mounts none.
  Ctrl-C stops a reply, and the line `/draw` logs a number from `math.random`.
- The code has no comments, by its owner's rule.
  This plan holds the reasons.

## Terms

- A **record** is one effect, answer, or event, as the Harness hands it to a recorder (`crates/harness-internal/runner/src/recorder.rs` lines 80 to 107).
- The **journal** is the Turso file that holds every record of each run, in the order the Harness appended them.
- A **full answer** is everything needed to build the answer again.
  The Harness's own answer record is not: a model round that asked for tools keeps only the tool names (`crates/runvil-internal/runtime/src/execute/run/effect.rs` lines 568 to 578).
- A **resume** runs the prompt again in a new process over the journal of an unfinished run.
- The **replay** is the part of a resume that produces records the journal already holds.
- The **cursor** is the count of journal records the replay has produced again.
- An answer's **turn** is its position in the journal.
  The replay gives an answer back when the cursor reaches its turn.
- **Live** is everything after the replay: effects are performed, and records are appended.
- A replay **diverges** when the record it produces differs from the journal's record at the cursor.

## What a resume does

1. The Host opens the journal and asks for its unfinished run (`crates/run-journal/src/journal.rs` lines 30 to 35).
2. `Journal::resume` reads the run's records and builds a `Session` (lines 53 to 70).
3. The Host builds its `RunSetup` and its `Harness` through the session, which builds the `Host` itself (`crates/run-journal/src/session.rs` lines 33 to 61).
4. The Harness runs the prompt from its first line.
5. Each record the run produces is compared with the journal's record at the cursor (`session.rs` lines 205 to 246).
6. Each effect that has an answer in the journal gets that answer at its turn.
7. Each effect that has no answer in the journal waits for the replay to end, and then it is performed.
8. A difference stops the run with an error that names both records, and the journal stays as it was.
9. After the last journal record, the run is live and appends to the same run.

## Where each answer comes from

| The run asks for          | The Host's seam                      | On replay                                                             |
| ------------------------- | ------------------------------------ | --------------------------------------------------------------------- |
| A model round             | `InferenceBroker::chat`              | The stored completion (`crates/run-journal/src/broker.rs`)            |
| A tool call               | `Pack::call`                         | The stored output or error (`crates/run-journal/src/packs.rs`)        |
| A render of offered tools | `Pack::render`                       | The tools the journal's answer holds                                  |
| A timer                   | `Timer::sleep`                       | Returns at its turn, with no wait (`crates/run-journal/src/timer.rs`) |
| The run's start time      | `Timer::now`                         | The recorded start                                                    |
| An operator message       | `RunControl::send`                   | Sent again from its `inbox_message` event                             |
| A stop                    | `RunControl::stop_round`             | Raised again when a dropped effect's turn comes                       |
| A store operation         | None: the Harness performs it inline | Performed again, and its answer compared                              |

## What the journal stores

- `runs`: one row per run, with the prompt's name, the arguments, the selected model, the seed, the start time, and how the run ended (`crates/run-journal/src/db.rs` lines 160 to 170).
- `records`: every record in append order, with its task, its position in the task, its kind, and its payload (lines 172 to 182).
- `records.answer`: the full answer of a model round or a tool call, beside its answer record (line 180).
- A run with no end time is unfinished, and only an unfinished run resumes.

## Decisions

- **A crate over the facade alone.**
  The library names `runvil` and no other Runvil crate.
  The facade leaves stepping to `runvil-engine` (`crates/runvil/src/lib.rs` lines 6 to 7), so the library cannot hand an `EffectAnswer` to a run.
  It answers at the seams a Host owns instead, which is the same answer one layer out.
- **Replay follows the journal's order, not each task's.**
  The Harness records in one order across tasks, and an answer that lands changes what the next step does.
  The replay releases one answer at a time, at its turn, so the records come out in the journal's order.
  This works because the loop takes every answer that is ready before it steps again (`crates/harness-internal/runner/src/effect_loop.rs` lines 405 to 410 and 827 to 832).
- **A call finds its effect record by what the call holds.**
  A model round carries its round id, which its effect record holds too.
  A tool call is paired by tool, arguments, and origin, oldest first.
  A render is paired by its section entry, and a timer by its seconds.
  The effect record is always appended before the call starts (`effect_loop.rs` lines 255 and 272).
- **Full answers are stored for model rounds and tool calls only.**
  A render's answer record holds its tools whole, and a timer's answer has no content.
  The full answer is written in the same insert as its answer record, so the two cannot part.
  A tool's full answer is kept only when it agrees with the answer record, so a wrong pairing never reaches the file (`crates/run-journal/src/answer.rs` lines 55 to 70).
- **Every replayed record is compared.**
  The comparison covers the kind, the task, the position, and the payload.
  It is the one defense against a changed prompt, a changed Pack, or a wrong pairing.
- **The guard nonce is blanked before the comparison.**
  The Harness draws a new seed for every run (`crates/harness-internal/runner/src/prepare.rs` line 46), and the nonce derives from the seed.
  The 32 digits after `untrusted_input_` are blanked on both sides (`crates/run-journal/src/normalize.rs` lines 12 to 36).
  Step 6 removes this.
- **A message is sent again when the cursor reaches its event.**
  An `inbox_message` on task `0` is always a message the Host sent, as the Harness's own plan says (`plan/2026-11-08-2-run-inbox-user-pack.plan.md` line 237).
  Messages that follow each other are sent together (`session.rs` lines 488 to 504).
- **A stop is inferred, not stored.**
  The Harness records no stop, only the dropped answers it causes.
  When a dropped effect's turn comes and the Harness has not dropped it, the replay raises a stop.
  A drop the Engine causes never reaches that point, because the Harness aborts the call before it polls it again (`effect_loop.rs` lines 416 to 421).
- **A run that ends during the replay stays open.**
  A Gateway that is down ends the run before its first step.
  The recorder refuses that end, so the journal keeps the run for a later resume (`session.rs` lines 187 to 203).
- **One journal, one unfinished run.**
  `Journal::unfinished` returns the latest run when it has no end.
  The command-line Host resumes it and refuses a prompt of another name.
- **Turso stays.**
  It is async, and Workshop's run log already uses the same version with the same features off (`runvil-workshop/Cargo.toml` lines 115 to 121).
  That manifest says the default features bring a C build; whether the rest of Turso builds native code is not checked.
- **One `Host` for each run.**
  The Packs and the timer sit on `Host`, so each run that a journal serves gets its own.
- **The model catalog is read live.**
  A resumed run binds whatever the broker lists now.
  Step 8 changes this.
- **The command-line Host forwards a line only while the run waits.**
  A line typed during the replay would arrive as a message the journal does not hold.
  When its input ends and the run waits, it exits and leaves the run open.
- **Ctrl-C stops the reply, and Ctrl-C again leaves the run.**
  While the run works, the first Ctrl-C calls `RunControl::stop_round`, which drops the round or the tool calls in flight and lets the run go on (`crates/resumable/src/main.rs` lines 117 to 128).
  A Ctrl-C during the replay, while the run waits, or after a stop that has not yet made the run wait exits with code 130 and leaves the run open.
  The Host raises no stop during the replay, because the journal would not hold it.
  The journal stores no stop: the dropped answer replays it.
  A line typed before the Ctrl-C waits in the Host, and it is sent when the run waits again.
- **The prompt catches the failure of a reply and logs its code.**
  A stop raises an error with the code `canceled` at `models.loop`, and the round cap raises one with the code `tool_loop_exhausted`.
  Uncaught, each ends the run, and a run that ended cannot resume.
  `prompts/agent.md` calls `models.loop` under `pcall`, as Workshop's chat prompt does (`runvil-workshop/crates/workshop/agents/agents/chat.md` line 52), and gives the error's code to `log` (lines 48 to 51).
  The screen shows each `log` line under `note> `, live and replayed (`crates/resumable/src/screen.rs` lines 77 to 83).
  The next message starts another `models.loop` call, with its own 24 rounds.
- **`/draw` in `prompts/agent.md` shows what `math.random` does to a resume.**
  The line `/draw` is not sent to the model: the prompt logs one number from `math.random` (lines 44 to 45).
  Lua seeds its generator in each process, so the replay logs another number, and the resume is refused at that record.
  A number that the model makes up is part of its answer, so it replays from the journal and shows nothing of this limit.
  A prompt that needs draws that replay seeds the generator from `sys.when`, the run's start time, which the journal gives back on a resume: `math.randomseed(tonumber((sys.when:gsub('%D', ''))))`.
- **One real directory is mounted read-only at its own path.**
  The path the operator types, the path the model gives a tool, and the path on the disk are then one path.
  The rest of the run's files stay in memory, so a real path outside the directory reads as not found.
  Workshop's demo mounts are built the same way (`runvil-workshop/crates/workshop/server/src/agents/language_guide.rs` lines 62 to 65).
  The directory is the run's first workspace root, so a prompt reads it from `ui().workspace_root`.
- **The journal does not store the mount.**
  A replayed tool answer comes from the journal, so the replay reads no real file and fetches no page.
  A resume takes `RUNVIL_FILES` from its own command, and the comparison refuses a resume whose prompt names another directory.
- **The web Pack is linked into the command-line Host.**
  `plugin-web` is a crate of the Harness repository, and Workshop links it the same way (`runvil-workshop/crates/workshop/server/src/agents.rs` line 172).
  A fetch leaves from the command-line Host's own process, and a search goes through the Gateway.
- **`run.sh` gives a run real files and web pages by default.**
  With no prompt named, it runs `prompts/agent.md`.
  With `RUNVIL_FILES` not set, it mounts `~/Downloads` when that directory exists.
  A set `RUNVIL_FILES` is left as it is, and an empty one mounts nothing, as an empty `RUNVIL_PLUGINS` loads nothing.
  The command-line Host has no default of its own: it mounts only the directory that `RUNVIL_FILES` names.
  The limit on a mounted directory and `web/fetch` in one run is therefore the default case.
- **`prompts/agent.md` fetches the whole page when the article is thin.**
  `web/fetch` returns the main article of a page, and renders the whole page only when the article is under 100 bytes (`crates/plugin-web/src/response.rs` lines 15 and 162).
  For `https://github.com/SomeoneToIgnore` the article is 235 characters about one pinned repository, and the name and the place are only in the whole page.
  The prompt offers `web_fetch`, a tool in Lua that calls `web/fetch` (`prompts/agent.md` lines 14 to 35).
  It calls `web/fetch` again with `raw` when the first answer is an article that is not cut and has at most 2000 characters, its envelope included.
  The system message tells the model to ask for `raw` itself when a longer article lacks what it looks for, and not to repeat a call.
  The directory and this text are one system message, because a model's chat template can refuse a second one; that refusal is not checked.
  A failure of `web/fetch` returns to the model as untrusted text, as it does when the model calls the Pack's tool.
  An error that a Lua tool raises ends `models.loop` instead, and with it the reply (`decisions/tools-and-packs.md`, "A failing adhoc handler raises at `models.loop`").

## Limits

| Limit                                                          | Worst case                                                                                                     | What the operator sees                           | Checked            |
| -------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ | ------------------ |
| Effects in flight at the exit are performed again              | 10 tool calls, the default cap (`crates/harness-internal/runner/src/setup.rs` line 16), plus rounds and timers | A tool that is not idempotent runs twice         | Test, for a round  |
| A message or a stop that arrived in the same wake as an answer | The replay orders them the other way                                                                           | The resume is refused, and the journal is intact | Reasoned           |
| Untrusted text that quotes the run's own nonce                 | The records differ after blanking                                                                              | The resume is refused                            | Reasoned           |
| `math.random` in a prompt                                      | Lua draws its own seed in each process                                                                         | The resume is refused when a record changes      | Seen in a real run |
| Two calls alike in one round                                   | Paired by issue order                                                                                          | Each gets its own answer                         | Test               |
| The model catalog                                              | Another model is bound                                                                                         | Later rounds go to that model                    | Reasoned           |
| The run's files and the shell session                          | They live in memory                                                                                            | A file the run wrote is gone                     | Seen in a real run |
| The journal is read whole at a resume                          | Memory grows with the run                                                                                      | A slow start for a long run                      | Reasoned           |
| A mounted directory and `web/fetch` in one run, the default    | Text in a file or a page directs the model to put a file's content into a URL                                  | Any file under the mount can leave the machine   | Reasoned           |
| A real file or a page that changed since it was read           | The replay gives back the content the journal holds                                                            | The model works from the old content             | Seen in a real run |
| A resume whose mount differs from the first start's            | The system message that names the directory differs                                                            | The resume is refused, and the journal is intact | Seen in a real run |
| `RUNVIL_FILES` on Windows                                      | The resolved path does not start with `/`                                                                      | The start is refused with the reason             | Reasoned           |
| An article of at most 2000 characters                          | A second request, and a whole page of up to 40,000 characters in place of the article                          | A slower round: 60 s for 23,000 characters       | Seen in a real run |
| A longer article that lacks what the operator asked for        | The model must ask for `raw` itself: 3 of 3 runs did with the system message, and 1 of 3 without it            | A wrong answer, or a reply that ends at the cap  | Seen in a real run |
| A tool loop that does not converge                             | 24 rounds, the default cap of one `models.loop` call                                                           | The reply ends with a note, and the run goes on  | Seen in a real run |
| A stop that finds no effect in flight                          | The Harness lowers the stop, and the reply goes on                                                             | The next Ctrl-C exits and leaves the run open    | Reasoned           |
| A prompt that does not catch the stop                          | The dropped call's error ends the run canceled                                                                 | The run ends, and the next start is a new run    | Reasoned           |
| A run in which the operator typed `/draw`                      | Each resume logs another number                                                                                | The resume is refused, and the journal is intact | Seen in a real run |
| More than 1024 `log` lines in one run                          | The Engine refuses the next `log` call, and the prompt drops that note                                         | A draw or an early end shows no note             | Reasoned           |
| Other header lines from `web/fetch`                            | The Lua tool finds no `extraction: readability` line                                                           | No second fetch, as before the Lua tool          | Reasoned           |
| A line half typed when other output or the prompt is printed   | The terminal does not echo the first half again                                                                | The two halves are on two rows, live only        | Seen in a real run |
| A reply with a line that starts with a role                    | Any line after the first of any reply; the text is printed as the model gave it                                | That line reads as another role's                | Reasoned           |

## Steps

### Step 1, done: the journal file and its recorder

- `crates/run-journal/src/db.rs` holds the schema and the statements.
- `crates/run-journal/src/journal.rs` opens the file, finds the unfinished run, reads a run's records, and starts or resumes a session.
- `crates/run-journal/src/recorder.rs` is the `RunRecorder`.
  It begins a row for a new run, hands back the old run's id for a resume, and closes the run at its end.
- Covered by `a_finished_run_is_closed_and_the_next_start_is_a_new_run` (`crates/run-journal/tests/it/resume.rs` line 307).

### Step 2, done: full answers

- `crates/run-journal/src/answer.rs` turns a completion, a tool output, or an error into JSON and back.
- A completion keeps its model, finish reason, reasoning text, reply or tool calls with ids and arguments, metrics, and diagnostics.
- An error keeps its code, message, prose, location, and facts.
- Covered by `two_calls_alike_and_a_failed_round_each_get_their_own_answer_back` (line 51), which holds a failed round, reasoning text, and a float that a plain parser reads back wrong.

### Step 3, done: replay

- `crates/run-journal/src/session.rs` holds the cursor, the pairing, and the comparison.
- `broker.rs`, `packs.rs`, and `timer.rs` in the same folder wrap the Host's broker, each Pack's factory, and the timer.
  Each wrapper claims its effect's record and gives the claim to `Shared::answer` (`session.rs` lines 320 to 353).
  That one function waits for a stored answer's turn, raises the stop for a dropped answer, and holds a live effect until the replay ends.
  It takes the live effect as a function, so a replayed effect never calls the Host's broker, Pack, or timer.
  A render keeps its own match, because an answer it cannot read is rendered again at its turn (`packs.rs` lines 91 to 104).
- `Session::harness` builds the `Host` over the wrapped Packs and timer, parses the launch's prompt, and keeps the run's control, which the replay needs for messages and stops.
  The session hands out no `Host`, so a Host cannot build a `Harness` over those Packs that the replay does not control.
- Covered by the tests of step 4.

### Step 4, done: each resume against an uninterrupted run

Each test runs one script twice.
The first run goes from start to end in one process with no journal.
The second run stops part way, and a new session resumes it.
The journal must then hold the same records as the first run, and the resumed run's model requests must equal the first run's.

- A conversation with a tool call (line 9).
- Two calls alike in one round, and a failed round (line 51).
- A resume under another prompt, which must stop at record 5 with both records named (line 91).
- A resume that cannot list models, which must leave the run open (line 134).
- A round in flight at the exit, with a message sent before the run started (line 165).
- A stopped round (line 190).
- A message that steered a round (line 230).
- Two tasks, a timer that fires, a timer that is canceled, and the store (line 268).
- A run that finishes (line 307).
- A round in flight in another task, which must wait for the replay to end (line 331).

### Step 5, done: the command-line Host and `run.sh`

- `crates/resumable/src/main.rs` resumes the journal's unfinished run or starts a new one, and reads the operator's lines from its input.
- `crates/resumable/src/screen.rs` prints each message, reply, tool result, and `log` line under its role, replayed or live: `user> `, `assistant> `, `tool> ` with the tool's alias, and `note> `.
  When the input and the output are both a terminal, it prints `user> ` as a prompt when the run waits, and the terminal's own echo completes the line.
  A line typed before the run waits is printed again under `user> ` when the run takes it.
  In every other case the whole line is printed when its message arrives, so a replay and a pipe show the same text as a terminal.
  Output that arrives while the prompt is open ends the prompt's row first, and the prompt is printed again after it.
- `RUNVIL_GATEWAY_URL` and `RUNVIL_GATEWAY_API_KEY` name the Gateway, `RUNNER_MODEL` selects a model, and `RUNVIL_PLUGINS` names a directory of library Plugins.
- `run.sh` fills those from the Workshop checkout beside this repository.
  It asks `runvil-gateway diagnostics` first and stops when no Gateway runs, because `--print-url` with no Gateway becomes one and never returns.

### Step 6, open: the recorded seed

- Give `RunSetup` a seed that `prepare::begin` uses instead of drawing one.
- The journal already stores the seed in `runs.seed`.
- Then remove the blanking in `normalize.rs` and compare payloads as they are.
- This is a Harness change, so it starts from `runvil-harness` at `master`.

### Step 7, open: the run's files

- Give each run a directory under the journal's folder through `RunSetup.vfs`.
- `RealBackend::rooted` exists for read-only mounts; whether it serves as a writable root is not checked.

### Step 8, open: the model catalog

- Store the model the run was bound to, and hand it back from `InferenceBroker::models` on a resume.
- `ModelInfo` has no serde form, so the journal needs its own.

### Step 9, open: Workshop

- Workshop at `master` does not build against the Harness at `master`: `Harness::new` takes `Arc<Prompt>`, and `crates/workshop/agents/src/conversation-run.rs` lines 51 and 121 pass a `Prompt`.
- Workshop shares one `Host` across conversations, so it needs a `Host` per conversation or replay inside the Harness.
- The UI must keep its session id across a relaunch.

### Step 10, open: the crate's home

- Its name must not start with `runvil-` or `harness-`, which mean the Engine and the Harness.
- The Harness holds no database, so the crate is a Host's library wherever it lands.

### Step 11, done: real files and web pages

- `RUNVIL_FILES` names one directory.
  `crates/resumable/src/main.rs` resolves it, mounts it read-only at its resolved path over the in-memory root, and makes it the run's first workspace root (lines 197 to 221).
- The same file installs the web Pack with the runtime's handle and the Gateway's search (lines 233 to 254).
  With no Gateway named, the Pack is unavailable, and a prompt that requires it is refused.
- `prompts/agent.md` offers `std/read`, `std/dir`, `std/grep`, and `web_fetch`, its tool in Lua over `web/fetch`, and names the directory in its system message.
  It offers no `web/search`, because the Gateway answers a search only when its `gateway.toml` has `[tools.web_search]`.
- `run.sh` makes `RUNVIL_FILES` absolute before it changes directory.
- `run.sh` runs `prompts/agent.md` when no prompt is named (line 12), and mounts `~/Downloads` when `RUNVIL_FILES` is not set and that directory exists (lines 31 to 33).
- `prompts/files.md` is removed.
  It offered the whole std Pack, which adds to the three tools of `prompts/agent.md` only the four that answer "not implemented".
- `prompts/chat.md` is removed.
  Its loop was the last four lines of `prompts/agent.md` then, and no code or test read it.
  The one prompt left declares `std` and `web`, so a run of it needs both Packs.
- Covered by `a_mounted_directory_is_read_only_at_its_own_path` (`crates/resumable/src/main.rs` line 315).
  The library is unchanged.

### Step 12, done: stop a reply, and keep the run when a reply ends early

- `crates/resumable/src/main.rs` takes Ctrl-C through `tokio::signal`, a feature of `tokio` that brings `signal-hook-registry` into `Cargo.lock`.
  The first Ctrl-C while the run works stops the round, and any other Ctrl-C leaves the run (lines 117 to 128).
- `crates/resumable/src/screen.rs` shows each `log` line under `note> ` (lines 77 to 83).
- `prompts/agent.md` calls `models.loop` under `pcall`, logs the code of a reply that ended early, and logs a draw for the line `/draw` (lines 42 to 53).
- The screen and the Ctrl-C path have no test in the crate, and the runs under Verification cover them.
  The two tests of `screen.rs` are removed: they sent the screen events that the tests built themselves, and one of them also wrote the terminal's echo into the output it then compared.
  So they checked the screen against its own picture of the Harness and of a terminal, and a run at a pseudo-terminal checks it against both.
  This step does not change the library.

## Verification

Run on 2026-11-18:

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace` pass, with 9 scenarios and 1 unit test.
- The scenarios passed 60 times in a row.
- Two deliberate breakages each failed a test: comparing without blanking the nonce, and pairing a tool call with the newest effect instead of the oldest.
- Against a running Gateway and the model `Ornith-1.5-9B-Q4_K_M`, three processes held one conversation over one journal.
  The third answered 94 when asked to double the number the first was told.
- A resume with no new input replayed that conversation in 0.03 s, with no model round.
- With the std Plugin loaded, a run of three tool calls, one of them an error, replayed in 0.04 s.
- `run.sh` started a run, resumed it, ran `prompts/files.md`, since removed, from another directory, and refused to start while no Gateway ran.
- Typing at a terminal is not checked; every run above read its lines from a pipe.

Run on 2026-11-18, for step 11:

- The same three commands pass, with 9 scenarios and 2 unit tests.
- One deliberate breakage failed the new test: mounting the directory writable.
- Through `run.sh`, the same model read a real file and named the codename in it.
- The file was then changed, and the resume printed the old codename, read a link that leaves the directory and got a refusal, and fetched `https://example.com` and named its title.
- A resume with no new input replayed those three tool calls and took 2.3 s through `run.sh`, with `cargo run` and the two questions to the Gateway included.
- A resume with another directory, and one with none, each stopped at record 9 with both records named, and the next resume with the first directory passed.
- A prompt with no model called the tools from Lua: `std/dir`, `std/grep`, and `std/read` answered inside the directory, and a `..` path, the link, `/etc/hosts`, and the link's target each read as refused or not found.
- A copy of a journal that an earlier build wrote for `prompts/chat.md` resumed under this build.
- `web/search` is not checked beyond the Gateway's refusal: this machine's Gateway has no search configured.

Run on 2026-11-18, for the roles on the screen:

- The same three commands pass, with 9 scenarios and 4 unit tests.
- Four deliberate breakages each failed a test: a typed line shown twice, no wake after an interrupted prompt, a prompt without a terminal, and a reply with no role.
- Through `run.sh` at a pseudo-terminal, the same model answered a line typed at the prompt and a line typed while it worked, and a blank line and the end of the input each ended the prompt's row.
- The resume of that run showed the same blocks under the same roles from a pipe, at a pseudo-terminal, and with the output in a file.
- `prompts/agent.md` showed `tool> Read` between the message and the reply, live and resumed.
- A prompt whose second task answers while the first waits ended the open prompt's row, printed the reply, and printed the prompt again.
- A copy of this repository's `chat.db`, which an earlier build wrote, resumed with 8 `user> ` and 8 `assistant> ` blocks, as many as its `inbox_message` and `assistant_reply` records.
- A line half typed when the reply arrived was split over two rows live, and its resume showed it whole.
- A person at a real terminal is not checked; a script typed every line above.

Run on 2026-11-18, for the defaults of `run.sh`:

- No Rust file changed, and the same three commands pass, with 9 scenarios and 4 unit tests.
- `run.sh` with no argument and no `RUNVIL_FILES`, started from another directory, named `~/Downloads` as the real files and opened `agent.db` in the root.
- Through `run.sh` with no `RUNVIL_FILES`, the same model listed `~/Downloads` and answered 5, the count `ls -A` gives, and fetched `https://github.com/SomeoneToIgnore/`.
- The resume of that run replayed both tool calls, a resume with an empty `RUNVIL_FILES` stopped at record 9 with both records named, and the next resume with the default passed.
- A copy of this repository's `chat.db` resumed under `prompts/chat.md` with `~/Downloads` mounted, and with no mount.
- With a stub in place of `cargo`, `run.sh` mounted nothing for an empty `RUNVIL_FILES`, for a home with no `Downloads`, and for no `HOME`, and it made a relative `RUNVIL_FILES` absolute.
- With an empty `RUNVIL_PLUGINS`, `prompts/agent.md` was refused with "std is unavailable: runvil/std is not installed".
- A person at a real terminal is not checked; a pipe gave every line above.

Run on 2026-11-18, for the fetch tool of `prompts/agent.md`:

- No Rust file changed.
- The operator asked for the name and the place on `https://github.com/SomeoneToIgnore/`, which `https://api.github.com/users/SomeoneToIgnore` gives as Kirill Bulatov and Turku, Finland.
- Before the change 0 of 3 runs named either, with the system message alone 2 of 3 named both, and with the Lua tool alone 3 of 3 did.
- The prompt as it stands, with `~/Downloads` mounted, read a real file and then named both in 3 of 3 runs, in 79 to 90 s each.
- Asked for the version on `https://www.rust-lang.org/`, whose article of 3118 characters lacks it, 3 of 3 runs answered 1.100.0.
  With the Lua tool and no system message 1 of 3 did, and one run failed after 24 rounds with "tool-call loop did not converge".
- A prompt with no model called the same Lua tool for 10 cases.
  A thin article became the whole page, with and without `max_chars`.
  A long article, a cut article, a `raw` call, a JSON body, and a 404 each returned as `web/fetch` gave them.
  A text that is not a URL, a call with no URL, and a `raw` that is not a boolean each returned as text and did not raise.
- The resume of one run replayed the Lua tool and its two calls of `web/fetch` in 2 s, with the same blocks on the screen.
- Each count above is of 3 runs with one model, `Ornith-1.5-9B-Q4_K_M`.

Run on 2026-11-18, for the tidy-up of the replay code:

- `cargo clippy --workspace --all-targets` and `cargo test --workspace` pass, with 10 scenarios and 4 unit tests.
- `cargo fmt --all --check` is not run, because `crates/resumable/src/main.rs` holds another change in progress.
  `rustfmt --check` passes on the other seven files this change touched, and in `main.rs` it reports only `interrupts`, which is that other change.
- The scenarios passed 60 times in a row.
- Three deliberate breakages each failed a test: a broker called before its claim is answered, a dropped answer that raises no stop, and a live effect that does not wait for the replay to end.
  The third failed no test before this change, and the new scenario covers it.
- No run against a Gateway is checked for this change.

Run on 2026-11-18, for step 12:

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace` pass, with 10 scenarios and 4 unit tests.
- One deliberate breakage failed the test: a screen that drops `log` lines.
- At a pseudo-terminal, Ctrl-C during a round printed the stop line and `note> the reply ended early: canceled`, the next line got its reply, and Ctrl-C at the prompt exited with code 130.
  The resume of that run showed the same blocks.
- With a signal in place of the key, a stop of a round and a stop of a `web/fetch` call inside the Lua tool each left one `Dropped` answer in the journal, and the resume of each showed the same blocks.
- A run killed in a round resumed, asked the round again, stopped at one signal, and answered the next message.
- A prompt that starts a slow fetch again after each drop took the stop at the first signal and exited with code 130 at the second.
- With `max_tool_iterations: 1` in a copy of the prompt, a fetch ended with `note> the reply ended early: tool_loop_exhausted`, and the next message got its reply in the same run.
- `/draw` printed `note> drawn: 851019`, the conversation went on, and the resume stopped at record 15 with both numbers named.
- A scratch prompt that seeds from `sys.when` logged the same draws in three resumes, one of them with a new draw after a message, and another journal drew other numbers.
- The operator's `agent.db` holds an open run that the `prompts/agent.md` of the commit before started, and its resume stops at record 5: the journal holds the Pack's `web/fetch` where the prompt now offers the Lua tool.
- A person at a real terminal is not checked.

Run on 2026-11-18, for the removal of the screen's tests:

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace` pass, with 10 scenarios and 2 unit tests.
- No run is checked again: the change removes test code only.

## Out of scope

- The run's in-memory files and the shell session of the bashkit Pack.
- Steering from the command line: a line typed while the run works waits for the run's next wait.
- More than one unfinished run in a journal.
- Trimming or compacting a journal.
- Writes to real files, and more than one mounted directory.

## Open questions

- Should store operations be replayed from the journal instead of performed again?
  They are performed again now, which rebuilds the store, and their answers are compared.
- Are the limits in the table accepted as they stand?
- Where does the crate live, and under what name?

## What the build found

- The facade has no stepping items, so "replay just replays `EffectAnswer`" holds one layer out, at the Host's seams.
- The Harness has no way to take a seed back, though `RunMeta::seed` says a replay passes it back (`crates/harness-internal/runner/src/recorder.rs` line 139).
- The Harness records no stop, so a replay can only infer one.
- With no model selected, the Harness binds the first model the broker lists (`crates/harness-internal/runner/src/environment.rs` lines 37 to 39).
- `runvil-gateway --print-url` with no Gateway running starts one on the operator's state directory and never returns.
- The std Plugin that Workshop pins answers `Write`, `StrReplace`, `Delete`, and `Glob` with "not implemented" (`runvil-plugin-std` at `782ad23e`, `crates/plugin-std/src/pack.rs` lines 97 to 102), so a prompt can list, read, and search, and cannot write.
- The Gateway answers `POST /v1/tools/web_search` with 404 and "tool not configured: web_search" until its `gateway.toml` has `[tools.web_search]`.
- A refused link is named by its path below the mount, as in "/link.txt escapes the mounted root", not by the path the model gave.
- `web/fetch` keeps any article of 100 bytes or more, however small a part of the page it is: 235 of 12,466 characters for a GitHub profile, and 176 of 19,127 for the front page of `lobste.rs`.
- The whole-page rendering of `web/fetch` includes the text of `script` and `style` elements: 23,325 characters for that profile, with the name at character 9,921, against 12,466 without them, with the name at character 17.
  A scratch program measured both over `htmd` 0.5.6 and `readabilityrs` 0.1.4, the versions in `Cargo.lock`.
- `tools.call` in Lua raises an invalid argument of `web/fetch` as a table with `kind`, `code`, and `message`, and returns a refused scheme or a 404 as text.
- `sys.when` is text such as `2026-11-18T15:17:21.578Z`, so a prompt takes its digits to seed `math.randomseed`.
- A terminal discards the output it has not yet shown when it takes Ctrl-C, so a script at a pseudo-terminal reads the screen before it sends the key.
