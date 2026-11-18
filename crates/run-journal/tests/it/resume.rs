use runvil::ErrorCode;

use crate::support::{
    BARE, DESK, ONCE, Phase, Reply, Step, TASKS, journal, journal_path, remove, unfinished,
    uninterrupted,
};

#[tokio::test]
async fn a_resumed_conversation_replays_its_answers_and_continues_where_it_stopped() {
    let path = journal_path("continues");
    let first = [
        Step::Wait(1),
        Step::Send("What do the notes say?"),
        Step::Wait(2),
        Step::Send("Thanks."),
        Step::Wait(3),
    ];
    let second = [Step::Send("When do we ship?"), Step::Wait(4)];
    let replies = [
        Reply::Read,
        Reply::Text("The notes say to ship on Friday."),
        Reply::Text("Noted."),
        Reply::Text("On Friday."),
    ];
    let whole = uninterrupted(
        DESK,
        &replies,
        &[first.as_slice(), second.as_slice()].concat(),
    )
    .await;

    let phase = Phase::journaled(&path, DESK, &replies[..3]).await;
    phase.play(&first).await;
    let before = phase.cut().await;
    assert_eq!((before.requests.len(), before.reads), (3, 1));

    let phase = Phase::journaled(&path, DESK, &replies[3..]).await;
    phase.replayed().await;
    assert_eq!((phase.requests(), phase.reads()), (0, 0));
    phase.play(&second).await;
    let after = phase.cut().await;

    assert_eq!(after.requests[..], whole.requests[3..]);
    assert_eq!(after.reads, 0);
    assert_eq!(after.records, whole.records);
    assert_eq!(journal(&path).await, whole.records);
    remove(&path);
}

#[tokio::test]
async fn two_calls_alike_and_a_failed_round_each_get_their_own_answer_back() {
    let path = journal_path("alike");
    let first = [
        Step::Wait(1),
        Step::Send("Read it twice."),
        Step::Wait(2),
        Step::Send("Now fail."),
        Step::Wait(3),
    ];
    let second = [Step::Send("Once more?"), Step::Wait(4)];
    let replies = [
        Reply::ReadTwice,
        Reply::Text("Both reads agree."),
        Reply::Fail,
        Reply::Text("No need."),
    ];
    let whole = uninterrupted(
        DESK,
        &replies,
        &[first.as_slice(), second.as_slice()].concat(),
    )
    .await;

    let phase = Phase::journaled(&path, DESK, &replies[..3]).await;
    phase.play(&first).await;
    let before = phase.cut().await;
    assert_eq!((before.requests.len(), before.reads), (3, 2));

    let phase = Phase::journaled(&path, DESK, &replies[3..]).await;
    phase.replayed().await;
    assert_eq!((phase.requests(), phase.reads()), (0, 0));
    phase.play(&second).await;
    let after = phase.cut().await;

    assert_eq!(after.requests[..], whole.requests[3..]);
    assert_eq!(journal(&path).await, whole.records);
    remove(&path);
}

#[tokio::test]
async fn a_resume_under_another_prompt_stops_at_the_first_record_that_differs() {
    let path = journal_path("diverges");
    let phase = Phase::journaled(
        &path,
        DESK,
        &[Reply::Read, Reply::Text("The notes say to ship on Friday.")],
    )
    .await;
    phase
        .play(&[
            Step::Wait(1),
            Step::Send("What do the notes say?"),
            Step::Wait(2),
        ])
        .await;
    phase.cut().await;
    let recorded = journal(&path).await;

    let phase = Phase::journaled(&path, BARE, &[]).await;
    let (report, outcome) = phase.ended().await;

    let error = report.unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (
            ErrorCode::RecorderFailed,
            concat!(
                "the run could not be recorded: the run recorder failed: ",
                "replay diverged at record 5 of run 1: the journal holds effect 0:5 ",
                r#"{"Render":{"alias":"default","entry":"0.0","offered":"#,
                r#"[{"dynamic_set":[],"pack":"desk","static_set":["desk/read"]}]}}, "#,
                "the run produced event 0:5 ",
                r#"{"kind":"tool_scope_validation_started","#,
                r#""provenance":{"seq":5,"task":"0"},"section":"Conversation"}"#,
            )
        )
    );
    assert_eq!(outcome.requests.len(), 0);
    assert_eq!(journal(&path).await, recorded);
    remove(&path);
}

#[tokio::test]
async fn a_resume_that_cannot_list_models_leaves_the_run_open() {
    let path = journal_path("unlisted");
    let phase = Phase::journaled(&path, DESK, &[Reply::Text("Hello.")]).await;
    phase
        .play(&[Step::Wait(1), Step::Send("Hello."), Step::Wait(2)])
        .await;
    phase.cut().await;
    let recorded = journal(&path).await;

    let phase = Phase::unlisted(&path, DESK).await;
    let (report, _) = phase.ended().await;

    let error = report.unwrap_err();
    assert_eq!(
        (error.code, error.message),
        (
            ErrorCode::RecorderFailed,
            format!(
                "the run could not be recorded: the run recorder failed: the run ended after \
                 replaying 0 of the journal's {} records, so the journal keeps the run open; \
                 it ended with the error: the model catalog could not be fetched: \
                 model access is turned off or not configured",
                recorded.len()
            )
        )
    );
    assert_eq!(journal(&path).await, recorded);
    remove(&path);
}

#[tokio::test]
async fn a_round_in_flight_at_the_exit_is_asked_again() {
    let path = journal_path("in-flight");
    let whole = uninterrupted(
        DESK,
        &[Reply::Text("Hello again.")],
        &[Step::Send("Hello."), Step::Wait(1)],
    )
    .await;

    let phase = Phase::journaled(&path, DESK, &[Reply::Stuck]).await;
    phase.play(&[Step::Send("Hello."), Step::Started(1)]).await;
    let before = phase.cut().await;
    assert_eq!(before.requests.len(), 1);

    let phase = Phase::journaled(&path, DESK, &[Reply::Text("Hello again.")]).await;
    phase.replayed().await;
    phase.play(&[Step::Wait(1)]).await;
    let after = phase.cut().await;

    assert_eq!(after.requests, whole.requests);
    assert_eq!(journal(&path).await, whole.records);
    remove(&path);
}

#[tokio::test]
async fn a_stopped_round_is_stopped_again_on_replay() {
    let path = journal_path("stopped");
    let first = [
        Step::Wait(1),
        Step::Send("First."),
        Step::Started(1),
        Step::Stop,
        Step::Wait(2),
        Step::Send("Second."),
        Step::Wait(3),
    ];
    let second = [Step::Send("Third."), Step::Wait(4)];
    let replies = [
        Reply::Stuck,
        Reply::Text("Second answer."),
        Reply::Text("Third answer."),
    ];
    let whole = uninterrupted(
        DESK,
        &replies,
        &[first.as_slice(), second.as_slice()].concat(),
    )
    .await;

    let phase = Phase::journaled(&path, DESK, &replies[..2]).await;
    phase.play(&first).await;
    phase.cut().await;

    let phase = Phase::journaled(&path, DESK, &replies[2..]).await;
    phase.replayed().await;
    assert_eq!(phase.requests(), 0);
    phase.play(&second).await;
    let after = phase.cut().await;

    assert_eq!(after.requests[..], whole.requests[2..]);
    assert_eq!(journal(&path).await, whole.records);
    remove(&path);
}

#[tokio::test]
async fn a_message_that_steered_a_round_steers_it_again_on_replay() {
    let path = journal_path("steered");
    let first = [
        Step::Wait(1),
        Step::Send("First."),
        Step::Started(1),
        Step::Send("Also this."),
        Step::Wait(2),
    ];
    let second = [Step::Send("Later?"), Step::Wait(3)];
    let replies = [
        Reply::Stuck,
        Reply::Text("Both read."),
        Reply::Text("Later."),
    ];
    let whole = uninterrupted(
        DESK,
        &replies,
        &[first.as_slice(), second.as_slice()].concat(),
    )
    .await;

    let phase = Phase::journaled(&path, DESK, &replies[..2]).await;
    phase.play(&first).await;
    phase.cut().await;

    let phase = Phase::journaled(&path, DESK, &replies[2..]).await;
    phase.replayed().await;
    assert_eq!(phase.requests(), 0);
    phase.play(&second).await;
    let after = phase.cut().await;

    assert_eq!(after.requests[..], whole.requests[2..]);
    assert_eq!(journal(&path).await, whole.records);
    remove(&path);
}

#[tokio::test]
async fn tasks_timers_and_the_store_replay_in_their_recorded_order() {
    let path = journal_path("tasks");
    let first = [
        Step::Wait(1),
        Step::Release,
        Step::Event("task_succeeded", 2),
        Step::Send("Hello."),
        Step::Wait(2),
    ];
    let second = [Step::Send("Again."), Step::Wait(3)];
    let replies = [
        Reply::Held("Slow answer."),
        Reply::Text("Hello there."),
        Reply::Text("Again there."),
    ];
    let whole = uninterrupted(
        TASKS,
        &replies,
        &[first.as_slice(), second.as_slice()].concat(),
    )
    .await;

    let phase = Phase::journaled(&path, TASKS, &replies[..2]).await;
    phase.play(&first).await;
    let before = phase.cut().await;
    assert_eq!(before.reads, 1);

    let phase = Phase::journaled(&path, TASKS, &replies[2..]).await;
    phase.replayed().await;
    assert_eq!((phase.requests(), phase.reads()), (0, 0));
    phase.play(&second).await;
    let after = phase.cut().await;

    assert_eq!(after.requests[..], whole.requests[2..]);
    assert_eq!(journal(&path).await, whole.records);
    remove(&path);
}

#[tokio::test]
async fn a_finished_run_is_closed_and_the_next_start_is_a_new_run() {
    let path = journal_path("finished");
    let phase = Phase::journaled(&path, ONCE, &[]).await;
    phase.play(&[Step::Send("Early.")]).await;
    let (report, _) = phase.ended().await;
    let report = report.unwrap();
    assert_eq!(
        (report.run_id.get(), report.result),
        (1, Ok("You said: Early.".to_owned()))
    );
    assert_eq!(unfinished(&path).await, None);

    let phase = Phase::journaled(&path, ONCE, &[]).await;
    phase.play(&[Step::Send("Again.")]).await;
    let (report, _) = phase.ended().await;
    let report = report.unwrap();
    assert_eq!(
        (report.run_id.get(), report.result),
        (2, Ok("You said: Again.".to_owned()))
    );
    remove(&path);
}
