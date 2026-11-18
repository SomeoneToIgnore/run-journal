use std::fmt;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use runvil::harness::{Record, RecordKind};
use runvil::{RunSeed, Timestamp};
use serde_json::Value;

use crate::error::JournalError;
use crate::journal::Launch;

pub(crate) struct Db {
    conn: turso::Connection,
}

pub(crate) struct RunRow {
    pub(crate) id: i64,
    pub(crate) launch: Launch,
    pub(crate) started_at: Timestamp,
    pub(crate) ended: bool,
}

pub(crate) struct Stored {
    pub(crate) record: Record,
    pub(crate) answer: Option<Value>,
}

impl Db {
    pub(crate) async fn open(path: &Path) -> Result<Db, JournalError> {
        let name = path.to_str().ok_or_else(|| JournalError::Io {
            context: format!("open {}", path.display()),
            source: io::Error::new(io::ErrorKind::InvalidInput, "journal path must be utf-8"),
        })?;
        let conn = turso::Builder::new_local(name).build().await?.connect()?;
        conn.execute_batch(SCHEMA).await?;
        Ok(Db { conn })
    }

    pub(crate) async fn begin(
        &mut self,
        launch: &Launch,
        seed: RunSeed,
        started_at: Timestamp,
    ) -> Result<i64, JournalError> {
        self.conn
            .execute(
                INSERT_RUN,
                (
                    launch.prompt.as_str(),
                    launch.args.as_str(),
                    launch.model.clone(),
                    seed.to_string(),
                    started_at.unix_millis(),
                ),
            )
            .await?;
        Ok(self.conn.last_insert_rowid())
    }

    pub(crate) async fn append(
        &mut self,
        run: i64,
        seq: u64,
        record: &Record,
        answer: Option<&Value>,
    ) -> Result<(), JournalError> {
        let payload = serde_json::to_string(&record.payload)?;
        let answer = answer.map(serde_json::to_string).transpose()?;
        self.conn
            .execute(
                INSERT_RECORD,
                (
                    run,
                    signed(seq),
                    record.task_id.as_str(),
                    i64::from(record.task_seq),
                    record.kind.as_str(),
                    record.effect_id.map(signed),
                    payload,
                    answer,
                ),
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn end(
        &mut self,
        run: i64,
        status: &str,
        result: &str,
    ) -> Result<(), JournalError> {
        let changed = self
            .conn
            .execute(UPDATE_RUN_ENDED, (run, now_ms(), status, result))
            .await?;
        if changed == 0 {
            return Err(JournalError::RunEnded(run));
        }
        Ok(())
    }

    pub(crate) async fn latest(&mut self) -> Result<Option<RunRow>, JournalError> {
        let mut rows = self.conn.query(SELECT_LATEST_RUN, ()).await?;
        match rows.next().await? {
            Some(row) => Ok(Some(run_row(&row)?)),
            None => Ok(None),
        }
    }

    pub(crate) async fn run(&mut self, run: i64) -> Result<RunRow, JournalError> {
        let mut rows = self.conn.query(SELECT_RUN, (run,)).await?;
        match rows.next().await? {
            Some(row) => run_row(&row),
            None => Err(JournalError::UnknownRun(run)),
        }
    }

    pub(crate) async fn records(&mut self, run: i64) -> Result<Vec<Stored>, JournalError> {
        let mut rows = self.conn.query(SELECT_RECORDS, (run,)).await?;
        let mut records = Vec::new();
        while let Some(row) = rows.next().await? {
            let kind = row.get::<String>(2)?;
            let kind = RecordKind::parse(&kind).ok_or_else(|| {
                JournalError::Corrupt(format!(
                    "expected kind effect, answer, or event, found {kind:?}"
                ))
            })?;
            let task_seq = row.get::<i64>(1)?;
            let task_seq = u32::try_from(task_seq).map_err(|_| {
                JournalError::Corrupt(format!("expected task_seq to fit u32, found {task_seq}"))
            })?;
            let answer = row
                .get::<Option<String>>(5)?
                .map(|text| serde_json::from_str::<Value>(&text))
                .transpose()?;
            records.push(Stored {
                record: Record {
                    task_id: row.get::<String>(0)?,
                    task_seq,
                    kind,
                    effect_id: row.get::<Option<i64>>(3)?.map(unsigned),
                    payload: serde_json::from_str::<Value>(&row.get::<String>(4)?)?,
                },
                answer,
            });
        }
        Ok(records)
    }
}

impl fmt::Debug for Db {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Db").finish_non_exhaustive()
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS runs (
    run_id     INTEGER PRIMARY KEY,
    prompt     TEXT    NOT NULL,
    args       TEXT    NOT NULL,
    model      TEXT,
    seed       TEXT    NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at   INTEGER,
    status     TEXT    CHECK (status IN ('completed', 'failed', 'canceled')),
    result     TEXT
);

CREATE TABLE IF NOT EXISTS records (
    run_id    INTEGER NOT NULL REFERENCES runs (run_id),
    seq       INTEGER NOT NULL,
    task_id   TEXT    NOT NULL,
    task_seq  INTEGER NOT NULL,
    kind      TEXT    NOT NULL CHECK (kind IN ('effect', 'answer', 'event')),
    effect_id INTEGER,
    payload   TEXT    NOT NULL,
    answer    TEXT,
    PRIMARY KEY (run_id, seq)
);
";

const INSERT_RUN: &str = "INSERT INTO runs \
    (prompt, args, model, seed, started_at) \
    VALUES (?1, ?2, ?3, ?4, ?5)";

const INSERT_RECORD: &str = "INSERT INTO records \
    (run_id, seq, task_id, task_seq, kind, effect_id, payload, answer) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

const UPDATE_RUN_ENDED: &str = "UPDATE runs SET \
    ended_at = ?2, status = ?3, result = ?4 \
    WHERE run_id = ?1 AND ended_at IS NULL";

const SELECT_LATEST_RUN: &str = "SELECT \
    run_id, prompt, args, model, started_at, ended_at \
    FROM runs ORDER BY run_id DESC LIMIT 1";

const SELECT_RUN: &str = "SELECT \
    run_id, prompt, args, model, started_at, ended_at \
    FROM runs WHERE run_id = ?1";

const SELECT_RECORDS: &str = "SELECT \
    task_id, task_seq, kind, effect_id, payload, answer \
    FROM records WHERE run_id = ?1 ORDER BY seq";

fn run_row(row: &turso::Row) -> Result<RunRow, JournalError> {
    Ok(RunRow {
        id: row.get::<i64>(0)?,
        launch: Launch {
            prompt: row.get::<String>(1)?,
            args: row.get::<String>(2)?,
            model: row.get::<Option<String>>(3)?,
        },
        started_at: Timestamp::from_unix_millis(row.get::<i64>(4)?),
        ended: row.get::<Option<i64>>(5)?.is_some(),
    })
}

const fn signed(value: u64) -> i64 {
    i64::from_le_bytes(value.to_le_bytes())
}

const fn unsigned(value: i64) -> u64 {
    u64::from_le_bytes(value.to_le_bytes())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}
