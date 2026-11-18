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
isProject: false
---

# A run resumes from its journal

Today a run lives only as long as its process.
When the process ends, the Engine's state ends with it, the conversation included.
This plan keeps a journal of the run in a file and resumes the run from it in a new process.
Nothing is restored: the run starts again from its first line, and every answer it had comes back from the journal instead of from a model or a tool.
The Engine is deterministic for the same inputs, so the run reaches the state it had.

Steps 1 to 5 are built and pass their checks.
Steps 6 to 10 are open.

## Notes

- Repository: `/Users/someonetoignore/work/cppalliance/run-journal`, a Rust workspace of two crates.
  Every path in this plan is relative to its root, unless the text names another repository.
- Crates: `crates/run-journal` is the library, and `crates/resumable` is the command-line Host.
- The Harness: `runvil` and `harness-gateway-client` come from `cppalliance/runvil-harness` at `db4cf1f4` (`Cargo.toml` lines 13 and 14).
  Harness paths in this plan are relative to that repository at that commit.
- Toolchain: Rust 1.99 (`rust-toolchain.toml`), as the Harness pins.
- Build: `cargo build --workspace`.
- Tests: `cargo test --workspace`.
- Lint: `cargo clippy --workspace --all-targets`.
  The lint table follows the Harness's (`Cargo.toml` lines 21 to 41).
  It leaves out `missing_docs`, the three rustdoc lints, the three `disallowed_*` lints, and the three lints about unsafe code beside `unsafe_code`.
  It allows `missing_errors_doc` and `missing_panics_doc`, because the code has no doc comments.
- Format: `cargo fmt --all --check`.
- Run: `./run.sh [prompt.md] [journal.db]`.
  With no arguments it runs `prompts/chat.md` over `chat.db` in the root.
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
3. The Host builds its `Host`, its `RunSetup`, and its `Harness` through the session (`crates/run-journal/src/session.rs` lines 34 to 64).
4. The Harness runs the prompt from its first line.
5. Each record the run produces is compared with the journal's record at the cursor (`session.rs` lines 208 to 249).
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
  The 32 digits after `untrusted_input_` are blanked on both sides (`crates/run-journal/src/normalize.rs` lines 27 to 53).
  Step 6 removes this.
- **A message is sent again when the cursor reaches its event.**
  An `inbox_message` on task `0` is always a message the Host sent, as the Harness's own plan says (`plan/2026-11-08-2-run-inbox-user-pack.plan.md` line 237).
  Messages that follow each other are sent together (`session.rs` lines 450 to 466).
- **A stop is inferred, not stored.**
  The Harness records no stop, only the dropped answers it causes.
  When a dropped effect's turn comes and the Harness has not dropped it, the replay raises a stop.
  A drop the Engine causes never reaches that point, because the Harness aborts the call before it polls it again (`effect_loop.rs` lines 416 to 421).
- **A run that ends during the replay stays open.**
  A Gateway that is down ends the run before its first step.
  The recorder refuses that end, so the journal keeps the run for a later resume (`session.rs` lines 190 to 206).
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
  Ctrl-C leaves the run open too, because nothing cancels it.

## Limits

| Limit                                                          | Worst case                                                                                                     | What the operator sees                           | Checked            |
| -------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ | ------------------ |
| Effects in flight at the exit are performed again              | 10 tool calls, the default cap (`crates/harness-internal/runner/src/setup.rs` line 16), plus rounds and timers | A tool that is not idempotent runs twice         | Test, for a round  |
| A message or a stop that arrived in the same wake as an answer | The replay orders them the other way                                                                           | The resume is refused, and the journal is intact | Reasoned           |
| Untrusted text that quotes the run's own nonce                 | The records differ after blanking                                                                              | The resume is refused                            | Reasoned           |
| `math.random` in a prompt                                      | Lua draws its own seed in each process                                                                         | The resume is refused when a record changes      | Reasoned           |
| Two calls alike in one round                                   | Paired by issue order                                                                                          | Each gets its own answer                         | Test               |
| The model catalog                                              | Another model is bound                                                                                         | Later rounds go to that model                    | Reasoned           |
| The run's files and the shell session                          | They live in memory                                                                                            | A file the run wrote is gone                     | Seen in a real run |
| The journal is read whole at a resume                          | Memory grows with the run                                                                                      | A slow start for a long run                      | Reasoned           |

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
- `Session::harness` keeps the run's control, which the replay needs for messages and stops.
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

### Step 5, done: the command-line Host and `run.sh`

- `crates/resumable/src/main.rs` resumes the journal's unfinished run or starts a new one, and reads the operator's lines from its input.
- `crates/resumable/src/screen.rs` prints each message and reply, replayed or live.
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

## Verification

Run on 2026-11-18:

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace` pass, with 9 scenarios and 1 unit test.
- The scenarios passed 60 times in a row.
- Two deliberate breakages each failed a test: comparing without blanking the nonce, and pairing a tool call with the newest effect instead of the oldest.
- Against a running Gateway and the model `Ornith-1.5-9B-Q4_K_M`, three processes held one conversation over one journal.
  The third answered 94 when asked to double the number the first was told.
- A resume with no new input replayed that conversation in 0.03 s, with no model round.
- With the std Plugin loaded, a run of three tool calls, one of them an error, replayed in 0.04 s.
- `run.sh` started a run, resumed it, ran `prompts/files.md` from another directory, and refused to start while no Gateway ran.
- Typing at a terminal is not checked; every run above read its lines from a pipe.

## Out of scope

- The run's in-memory files and the shell session of the bashkit Pack.
- Steering from the command line: a line typed while the run works waits for the run's next wait.
- More than one unfinished run in a journal.
- Trimming or compacting a journal.

### Open questions

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
- The std Plugin that Workshop pins answers `Write` and `StrReplace` with "not implemented", so the files prompt can list and read, and cannot write.
