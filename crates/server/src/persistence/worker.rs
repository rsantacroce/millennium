//! The database thread.

use super::memory::MemoryStore;
use super::postgres::PgStore;
use super::sqlite::SqliteStore;
use super::{DbCommand, DbResult};
use crate::config::DatabaseConfig;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy)]
pub struct HistoryLimits {
    pub global: i64,
    pub whispers: i64,
}

enum Store {
    Postgres(PgStore),
    Sqlite(SqliteStore),
    Memory(MemoryStore),
}

pub type Channels = (mpsc::UnboundedSender<DbCommand>, crossbeam_channel::Receiver<DbResult>);

/// Connects (and migrates) synchronously so startup fails loudly if Postgres is unreachable,
/// then hands the store to a background thread.
pub fn spawn_worker(config: &DatabaseConfig, history: HistoryLimits) -> Result<Channels, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("db")
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let url = std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty()).unwrap_or(config.url.clone());
    // The URL scheme picks the store: postgres:// for a dedicated server, sqlite:// for a
    // self-contained save file (hosted games), empty for throwaway in-memory data.
    let store = if url.is_empty() {
        tracing::warn!("no database configured: using the in-memory store (data is lost on restart)");
        Store::Memory(MemoryStore::default())
    } else if url.starts_with("sqlite:") {
        let store = runtime.block_on(SqliteStore::open(&url)).map_err(|e| format!("{url}: {e}"))?;
        tracing::info!("opened save file {url}");
        Store::Sqlite(store)
    } else {
        let pg = runtime
            .block_on(PgStore::connect(&url, config.max_connections))
            .map_err(|e| format!("{e} (is Postgres running? try `docker compose up -d`)"))?;
        tracing::info!("connected to Postgres and applied migrations");
        Store::Postgres(pg)
    };

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<DbCommand>();
    let (res_tx, res_rx) = crossbeam_channel::unbounded::<DbResult>();

    std::thread::Builder::new()
        .name("db-worker".into())
        .spawn(move || {
            runtime.block_on(async move {
                let mut store = store;
                while let Some(cmd) = cmd_rx.recv().await {
                    let result = match &mut store {
                        Store::Postgres(pg) => pg.handle(cmd, history).await,
                        Store::Sqlite(db) => db.handle(cmd, history).await,
                        Store::Memory(mem) => mem.handle(cmd, history),
                    };
                    if let Some(result) = result
                        && res_tx.send(result).is_err()
                    {
                        break;
                    }
                }
                tracing::info!("database worker stopped");
            });
        })
        .map_err(|e| e.to_string())?;

    Ok((cmd_tx, res_rx))
}
