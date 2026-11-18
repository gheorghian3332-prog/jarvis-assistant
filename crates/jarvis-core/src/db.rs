use crate::service::{CoreError, CoreResult};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task;

/// Ordered schema migrations; entry `n` upgrades schema_version from `n` to `n + 1`.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE settings(
         key TEXT PRIMARY KEY NOT NULL,
         value TEXT NOT NULL,
         updated_at TEXT NOT NULL
     );
     CREATE TABLE audit_log(
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         ts TEXT NOT NULL,
         action TEXT NOT NULL,
         args TEXT NOT NULL,
         risk TEXT NOT NULL,
         ok INTEGER NOT NULL,
         detail TEXT NOT NULL
     );",
    // 2: the assistant's memory (P9-A).
    "CREATE TABLE command_history(
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         ts TEXT NOT NULL,
         text TEXT NOT NULL,
         lang TEXT NOT NULL,
         route TEXT NOT NULL,
         action TEXT,
         ok INTEGER NOT NULL
     );
     CREATE INDEX idx_command_history_ts ON command_history(ts);
     CREATE TABLE memory_facts(
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         ts TEXT NOT NULL,
         kind TEXT NOT NULL,
         key TEXT NOT NULL,
         value TEXT NOT NULL,
         UNIQUE(kind, key)
     );
     CREATE TABLE apps(
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         ts TEXT NOT NULL,
         name TEXT NOT NULL UNIQUE,
         target TEXT NOT NULL
     );
     CREATE TABLE permissions(
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         ts TEXT NOT NULL,
         action TEXT NOT NULL UNIQUE,
         allowed INTEGER NOT NULL
     );",
];

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub id: i64,
    pub ts: String,
    pub action: String,
    pub args: Value,
    pub risk: String,
    pub ok: bool,
    pub detail: String,
}

/// SQLite handle. All queries run on the blocking thread pool, never on the async thread.
#[derive(Clone, Debug)]
pub struct Database {
    connection: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl Database {
    /// Data directory: `JARVIS_DATA_DIR` if set, otherwise `%APPDATA%\Jarvis`.
    pub fn data_dir() -> CoreResult<PathBuf> {
        match env::var_os("JARVIS_DATA_DIR") {
            Some(path) => Ok(PathBuf::from(path)),
            None => env::var_os("APPDATA")
                .map(|appdata| PathBuf::from(appdata).join("Jarvis"))
                .ok_or_else(|| CoreError::Configuration("APPDATA is unavailable".into())),
        }
    }

    pub fn default_path() -> CoreResult<PathBuf> {
        Ok(Self::data_dir()?.join("jarvis.db"))
    }

    pub fn open(path: impl AsRef<Path>) -> CoreResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        migrate(&connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Directory for log files, next to the database.
    pub fn logs_dir(&self) -> CoreResult<PathBuf> {
        self.path
            .parent()
            .map(|parent| parent.join("logs"))
            .ok_or_else(|| CoreError::Configuration("database path has no parent".into()))
    }

    pub async fn schema_version(&self) -> CoreResult<i64> {
        self.run(read_version).await
    }

    pub async fn settings_get(&self, key: String) -> CoreResult<Option<Value>> {
        self.run(move |connection| {
            let text: Option<String> = connection
                .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                    row.get(0)
                })
                .optional()?;
            Ok(text.map(|text| serde_json::from_str(&text)).transpose()?)
        })
        .await
    }

    pub async fn settings_set(&self, key: String, value: Value) -> CoreResult<()> {
        self.run(move |connection| {
            connection.execute(
                "INSERT INTO settings(key, value, updated_at) VALUES(?1, ?2, datetime('now'))
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![key, serde_json::to_string(&value)?],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn audit_append(
        &self,
        action: String,
        args: Value,
        risk: String,
        ok: bool,
        detail: String,
    ) -> CoreResult<()> {
        self.run(move |connection| {
            connection.execute(
                "INSERT INTO audit_log(ts, action, args, risk, ok, detail)
                 VALUES(datetime('now'), ?1, ?2, ?3, ?4, ?5)",
                params![action, serde_json::to_string(&args)?, risk, ok, detail],
            )?;
            Ok(())
        })
        .await
    }

    /// Newest entries first.
    pub async fn audit_list(&self, limit: u32) -> CoreResult<Vec<AuditEntry>> {
        self.run(move |connection| {
            let mut statement = connection.prepare(
                "SELECT id, ts, action, args, risk, ok, detail FROM audit_log
                 ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = statement.query_map([i64::from(limit)], |row| {
                let args: String = row.get(3)?;
                let args = serde_json::from_str(&args).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(AuditEntry {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    action: row.get(2)?,
                    args,
                    risk: row.get(4)?,
                    ok: row.get(5)?,
                    detail: row.get(6)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    pub(crate) async fn run<T, F>(&self, job: F) -> CoreResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> CoreResult<T> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        task::spawn_blocking(move || {
            let guard = connection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            job(&guard)
        })
        .await
        .map_err(CoreError::join)?
    }
}

fn read_version(connection: &Connection) -> CoreResult<i64> {
    Ok(
        connection.query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
            row.get(0)
        })?,
    )
}

fn migrate(connection: &Connection) -> CoreResult<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL);
         INSERT INTO schema_version(version)
             SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM schema_version);",
    )?;
    let current = usize::try_from(read_version(connection)?)
        .map_err(|_| CoreError::Configuration("negative schema_version".into()))?;
    if current > MIGRATIONS.len() {
        return Err(CoreError::Configuration(format!(
            "database schema {current} is newer than supported {}",
            MIGRATIONS.len()
        )));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute_batch(sql)?;
        transaction.execute("UPDATE schema_version SET version = ?1", [index + 1])?;
        transaction.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = Database::open(directory.path().join("test.db")).expect("database");
        (directory, database)
    }

    #[tokio::test]
    async fn migrations_create_schema_and_are_idempotent() {
        let (directory, database) = temp_db();
        assert_eq!(database.schema_version().await.expect("version"), 2);
        drop(database);
        let reopened = Database::open(directory.path().join("test.db")).expect("reopen");
        assert_eq!(reopened.schema_version().await.expect("version"), 2);
        let tables = reopened
            .run(|connection| {
                let mut statement = connection
                    .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
                let names = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(names)
            })
            .await
            .expect("tables");
        assert!(tables.iter().any(|name| name == "settings"));
        assert!(tables.iter().any(|name| name == "audit_log"));
        assert!(tables.iter().any(|name| name == "schema_version"));
        for table in ["command_history", "memory_facts", "apps", "permissions"] {
            assert!(tables.iter().any(|name| name == table), "{table}");
        }
    }

    #[tokio::test]
    async fn uses_wal_mode() {
        let (_directory, database) = temp_db();
        let mode: String = database
            .run(|connection| {
                Ok(connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?)
            })
            .await
            .expect("journal mode");
        assert_eq!(mode, "wal");
    }

    #[tokio::test]
    async fn rejects_newer_schema() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("future.db");
        let database = Database::open(&path).expect("database");
        database
            .run(|connection| Ok(connection.execute("UPDATE schema_version SET version = 99", [])?))
            .await
            .expect("bump");
        drop(database);
        assert!(Database::open(&path).is_err());
    }

    #[tokio::test]
    async fn settings_round_trip() {
        let (_directory, database) = temp_db();
        assert_eq!(
            database.settings_get("theme".into()).await.expect("get"),
            None
        );
        database
            .settings_set("theme".into(), json!({"mode": "dark", "n": [1, 2]}))
            .await
            .expect("set");
        database
            .settings_set("other".into(), json!("1"))
            .await
            .expect("set other");
        assert_eq!(
            database.settings_get("theme".into()).await.expect("get"),
            Some(json!({"mode": "dark", "n": [1, 2]}))
        );
        database
            .settings_set("theme".into(), json!("light"))
            .await
            .expect("overwrite");
        assert_eq!(
            database.settings_get("theme".into()).await.expect("get"),
            Some(json!("light"))
        );
        assert_eq!(
            database.settings_get("other".into()).await.expect("get"),
            Some(json!("1"))
        );
    }

    #[tokio::test]
    async fn audit_append_and_list() {
        let (_directory, database) = temp_db();
        for name in ["a", "b", "c"] {
            database
                .audit_append(
                    name.into(),
                    json!({"k": name}),
                    "safe".into(),
                    true,
                    "d".into(),
                )
                .await
                .expect("append");
        }
        let entries = database.audit_list(2).await.expect("list");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].action, "c");
        assert_eq!(entries[1].args, json!({"k": "b"}));
        assert!(entries[0].ok);
    }
}
