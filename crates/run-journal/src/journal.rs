use std::path::Path;
use std::sync::Arc;

use runvil::harness::{Record, RunId};
use tokio::sync::Mutex;

use crate::db::Db;
use crate::error::JournalError;
use crate::session::{Resumed, Session};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub prompt: String,
    pub args: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Journal {
    db: Arc<Mutex<Db>>,
}

impl Journal {
    pub async fn open(path: &Path) -> Result<Journal, JournalError> {
        Ok(Journal {
            db: Arc::new(Mutex::new(Db::open(path).await?)),
        })
    }

    pub async fn unfinished(&self) -> Result<Option<(RunId, Launch)>, JournalError> {
        let latest = self.db.lock().await.latest().await?;
        Ok(latest
            .filter(|run| !run.ended)
            .map(|run| (RunId::from_raw(run.id), run.launch)))
    }

    pub async fn records(&self, run: RunId) -> Result<Vec<Record>, JournalError> {
        let mut db = self.db.lock().await;
        db.run(run.get()).await?;
        Ok(db
            .records(run.get())
            .await?
            .into_iter()
            .map(|stored| stored.record)
            .collect::<Vec<_>>())
    }

    #[must_use]
    pub fn start(&self, launch: Launch) -> Session {
        Session::new(Arc::clone(&self.db), launch, None)
    }

    pub async fn resume(&self, run: RunId) -> Result<Session, JournalError> {
        let mut db = self.db.lock().await;
        let row = db.run(run.get()).await?;
        if row.ended {
            return Err(JournalError::RunEnded(row.id));
        }
        let seed = row.seed()?;
        let records = db.records(row.id).await?;
        drop(db);
        Ok(Session::new(
            Arc::clone(&self.db),
            row.launch,
            Some(Resumed {
                run: row.id,
                started_at: row.started_at,
                seed,
                records,
            }),
        ))
    }
}
