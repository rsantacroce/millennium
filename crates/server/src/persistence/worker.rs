//! The database thread.

use super::postgres::PgStore;
use super::sqlite::{self, SqliteStore};
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
}

pub type Channels = (mpsc::UnboundedSender<DbCommand>, crossbeam_channel::Receiver<DbResult>);

/// Connects (and migrates) synchronously so startup fails loudly if Postgres is unreachable,
/// then hands the store to a background thread.
pub fn spawn_worker(config: &DatabaseConfig, history: HistoryLimits, stats: std::sync::Arc<crate::metrics::DbStats>) -> Result<Channels, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("db")
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let url = std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty()).unwrap_or(config.url.clone());
    // The URL scheme picks the store: postgres:// for a dedicated server, sqlite:// for a
    // self-contained save file (Host Game). Empty = an in-memory SQLite database (nothing is saved).
    let store = if url.is_empty() || url.starts_with("sqlite:") {
        let url = if url.is_empty() {
            tracing::warn!("no database configured: using an in-memory database (data is lost on restart)");
            "sqlite::memory:".to_string()
        } else {
            url.clone()
        };
        let store = runtime.block_on(SqliteStore::open(&url)).map_err(|e| format!("{url}: {e}"))?;
        tracing::info!("opened {url}");
        Store::Sqlite(store)
    } else {
        let pg = runtime
            .block_on(PgStore::connect(&url, config.max_connections))
            .map_err(|e| format!("{e} (is Postgres running? try `docker compose up -d`)"))?;
        tracing::info!("connected to Postgres and applied migrations");
        Store::Postgres(pg)
    };

    // Backups of a SQLite save file: at startup, then every interval, between commands.
    let backups = match &store {
        Store::Sqlite(_) => sqlite::file_of(&url).filter(|_| config.backup_interval_minutes > 0.0).map(|file| {
            let name = file.file_stem().map_or("world".into(), |s| s.to_string_lossy().into_owned());
            (std::path::PathBuf::from(&config.backup_dir), name, config.backup_keep)
        }),
        Store::Postgres(_) => {
            tracing::info!("Postgres: back it up with scripts/backup-postgres.sh (pg_dump)");
            None
        }
    };
    let backup_every = std::time::Duration::from_secs_f32(config.backup_interval_minutes.max(0.01) * 60.0);

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<DbCommand>();
    let (res_tx, res_rx) = crossbeam_channel::unbounded::<DbResult>();

    std::thread::Builder::new()
        .name("db-worker".into())
        .spawn(move || {
            runtime.block_on(async move {
                let mut store = store;
                // The first tick fires straight away: a backup at every startup.
                let mut backup_timer = tokio::time::interval(backup_every);
                backup_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    let cmd = tokio::select! {
                        cmd = cmd_rx.recv() => match cmd {
                            Some(cmd) => cmd,
                            None => break,
                        },
                        _ = backup_timer.tick(), if backups.is_some() => {
                            if let (Store::Sqlite(db), Some((dir, name, keep))) = (&store, &backups) {
                                match db.backup(dir, name, *keep).await {
                                    Ok(path) => tracing::info!("backed up to {}", path.display()),
                                    Err(e) => tracing::error!("backup failed: {e}"),
                                }
                            }
                            continue;
                        }
                    };
                    let started = std::time::Instant::now();
                    let result = match &mut store {
                        Store::Postgres(pg) => pg.handle(cmd, history).await,
                        Store::Sqlite(db) => db.handle(cmd, history).await,
                    };
                    stats.record(started.elapsed());
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
