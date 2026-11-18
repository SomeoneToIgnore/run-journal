use std::io;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JournalError {
    #[error("journal database")]
    Database {
        #[source]
        source: turso::Error,
    },
    #[error("journal file: {context}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("journal payload")]
    Payload {
        #[source]
        source: serde_json::Error,
    },
    #[error("journal: unknown run {0}")]
    UnknownRun(i64),
    #[error("journal: run {0} has ended, so it cannot be resumed")]
    RunEnded(i64),
    #[error("journal: corrupt row: {0}")]
    Corrupt(String),
}

impl From<turso::Error> for JournalError {
    fn from(source: turso::Error) -> Self {
        JournalError::Database { source }
    }
}

impl From<serde_json::Error> for JournalError {
    fn from(source: serde_json::Error) -> Self {
        JournalError::Payload { source }
    }
}
