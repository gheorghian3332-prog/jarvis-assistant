//! The assistant's memory in SQLite: command history, remembered facts, user-registered apps.
//!
//! Text search is done in Rust over a bounded number of rows because SQLite's `LOWER`/`LIKE` do not
//! fold Cyrillic case.

use crate::{db::Database, service::CoreResult};
use rusqlite::{params, OptionalExtension};

/// Facts kept at most; the oldest are replaced by new ones only through `forget`.
pub const MAX_FACTS: usize = 500;
/// History rows scanned by a text search.
const SEARCH_WINDOW: i64 = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub id: i64,
    pub ts: String,
    pub kind: String,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: i64,
    pub ts: String,
    pub text: String,
    pub lang: String,
    pub route: String,
    pub action: Option<String>,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewHistory {
    pub text: String,
    pub lang: String,
    pub route: String,
    pub action: Option<String>,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredApp {
    pub id: i64,
    pub name: String,
    pub target: String,
}

/// Lower-cased words of at least `min` characters.
pub fn words(text: &str, min: usize) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= min)
        .map(str::to_owned)
        .collect()
}

/// Crude stem for Russian and English: the first five letters.
fn stem(word: &str) -> String {
    word.chars().take(5).collect()
}

/// How many of `query_words` occur (by stem) in `haystack`.
pub fn relevance(query_words: &[String], haystack: &str) -> usize {
    let stems: Vec<String> = words(haystack, 2).iter().map(|w| stem(w)).collect();
    query_words
        .iter()
        .filter(|word| stems.contains(&stem(word)))
        .count()
}

fn fact_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
    Ok(Fact {
        id: row.get(0)?,
        ts: row.get(1)?,
        kind: row.get(2)?,
        key: row.get(3)?,
        value: row.get(4)?,
    })
}

fn history_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: row.get(0)?,
        ts: row.get(1)?,
        text: row.get(2)?,
        lang: row.get(3)?,
        route: row.get(4)?,
        action: row.get(5)?,
        ok: row.get(6)?,
    })
}

impl Database {
    // ---------- facts ----------

    /// Inserts a fact or replaces the value of the same `(kind, key)`. Returns its id, or `None`
    /// when the table is full and the fact is new.
    pub async fn fact_upsert(
        &self,
        kind: String,
        key: String,
        value: String,
    ) -> CoreResult<Option<i64>> {
        self.run(move |connection| {
            let existing: Option<i64> = connection
                .query_row(
                    "SELECT id FROM memory_facts WHERE kind = ?1 AND key = ?2",
                    params![kind, key],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                connection.execute(
                    "UPDATE memory_facts SET value = ?1, ts = datetime('now') WHERE id = ?2",
                    params![value, id],
                )?;
                return Ok(Some(id));
            }
            let count: i64 =
                connection.query_row("SELECT COUNT(*) FROM memory_facts", [], |row| row.get(0))?;
            if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_FACTS {
                return Ok(None);
            }
            connection.execute(
                "INSERT INTO memory_facts(ts, kind, key, value) VALUES(datetime('now'), ?1, ?2, ?3)",
                params![kind, key, value],
            )?;
            Ok(Some(connection.last_insert_rowid()))
        })
        .await
    }

    /// All facts, newest first, optionally only those that match the words of `query`.
    pub async fn fact_list(&self, query: Option<String>, limit: usize) -> CoreResult<Vec<Fact>> {
        let all = self
            .run(|connection| {
                let mut statement = connection.prepare(
                    "SELECT id, ts, kind, key, value FROM memory_facts ORDER BY id DESC",
                )?;
                let rows = statement.query_map([], fact_from)?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        let Some(query) = query.filter(|q| !q.trim().is_empty()) else {
            return Ok(all.into_iter().take(limit).collect());
        };
        let query_words = words(&query, 2);
        let mut scored: Vec<(usize, Fact)> = all
            .into_iter()
            .map(|fact| {
                let text = format!("{} {} {}", fact.kind, fact.key, fact.value);
                (relevance(&query_words, &text), fact)
            })
            .filter(|(score, _)| *score > 0)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.id.cmp(&a.1.id)));
        Ok(scored
            .into_iter()
            .map(|(_, fact)| fact)
            .take(limit)
            .collect())
    }

    /// The facts most relevant to `text` (by shared word stems), best first.
    pub async fn fact_relevant(&self, text: String, limit: usize) -> CoreResult<Vec<Fact>> {
        self.fact_list(Some(text), limit).await
    }

    pub async fn fact_get(&self, id: i64) -> CoreResult<Option<Fact>> {
        self.run(move |connection| {
            Ok(connection
                .query_row(
                    "SELECT id, ts, kind, key, value FROM memory_facts WHERE id = ?1",
                    [id],
                    fact_from,
                )
                .optional()?)
        })
        .await
    }

    pub async fn fact_delete(&self, id: i64) -> CoreResult<bool> {
        self.run(move |connection| {
            Ok(connection.execute("DELETE FROM memory_facts WHERE id = ?1", [id])? > 0)
        })
        .await
    }

    // ---------- history ----------

    pub async fn history_add(&self, entry: NewHistory) -> CoreResult<i64> {
        self.run(move |connection| {
            connection.execute(
                "INSERT INTO command_history(ts, text, lang, route, action, ok)
                 VALUES(datetime('now'), ?1, ?2, ?3, ?4, ?5)",
                params![entry.text, entry.lang, entry.route, entry.action, entry.ok],
            )?;
            Ok(connection.last_insert_rowid())
        })
        .await
    }

    /// Newest first; `query` filters by the words of the phrase or the action name.
    pub async fn history_list(
        &self,
        query: Option<String>,
        limit: usize,
    ) -> CoreResult<Vec<HistoryEntry>> {
        let filter = query.filter(|q| !q.trim().is_empty());
        let window = if filter.is_some() {
            SEARCH_WINDOW
        } else {
            i64::try_from(limit).unwrap_or(i64::MAX)
        };
        let rows = self
            .run(move |connection| {
                let mut statement = connection.prepare(
                    "SELECT id, ts, text, lang, route, action, ok FROM command_history
                     ORDER BY id DESC LIMIT ?1",
                )?;
                let rows = statement.query_map([window], history_from)?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        let Some(filter) = filter else {
            return Ok(rows);
        };
        let query_words = words(&filter, 2);
        Ok(rows
            .into_iter()
            .filter(|entry| {
                let text = format!("{} {}", entry.text, entry.action.as_deref().unwrap_or(""));
                relevance(&query_words, &text) > 0
            })
            .take(limit)
            .collect())
    }

    pub async fn history_delete(&self, id: i64) -> CoreResult<bool> {
        self.run(move |connection| {
            Ok(connection.execute("DELETE FROM command_history WHERE id = ?1", [id])? > 0)
        })
        .await
    }

    pub async fn history_clear(&self) -> CoreResult<usize> {
        self.run(|connection| Ok(connection.execute("DELETE FROM command_history", [])?))
            .await
    }

    /// Deletes rows older than `days` days; returns how many.
    pub async fn history_purge(&self, days: u32) -> CoreResult<usize> {
        self.run(move |connection| {
            Ok(connection.execute(
                "DELETE FROM command_history WHERE ts < datetime('now', ?1)",
                [format!("-{days} days")],
            )?)
        })
        .await
    }

    /// Test and tooling helper: moves a row into the past.
    pub async fn history_backdate(&self, id: i64, days: u32) -> CoreResult<()> {
        self.run(move |connection| {
            connection.execute(
                "UPDATE command_history SET ts = datetime('now', ?1) WHERE id = ?2",
                params![format!("-{days} days"), id],
            )?;
            Ok(())
        })
        .await
    }

    // ---------- apps registered by the user ----------

    pub async fn app_upsert(&self, name: String, target: String) -> CoreResult<()> {
        self.run(move |connection| {
            connection.execute(
                "INSERT INTO apps(ts, name, target) VALUES(datetime('now'), ?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET target = excluded.target, ts = excluded.ts",
                params![name, target],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn app_list(&self) -> CoreResult<Vec<RegisteredApp>> {
        self.run(|connection| {
            let mut statement =
                connection.prepare("SELECT id, name, target FROM apps ORDER BY name")?;
            let rows = statement.query_map([], |row| {
                Ok(RegisteredApp {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    target: row.get(2)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    pub async fn app_delete(&self, id: i64) -> CoreResult<bool> {
        self.run(move |connection| {
            Ok(connection.execute("DELETE FROM apps WHERE id = ?1", [id])? > 0)
        })
        .await
    }

    // ---------- everything ----------

    /// Facts, history and registered apps (the PRIVACY tab's "delete all memory").
    pub async fn memory_clear_all(&self) -> CoreResult<()> {
        self.run(|connection| {
            connection.execute_batch(
                "DELETE FROM memory_facts; DELETE FROM command_history; DELETE FROM apps;",
            )?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = Database::open(directory.path().join("test.db")).expect("database");
        (directory, database)
    }

    fn entry(text: &str) -> NewHistory {
        NewHistory {
            text: text.into(),
            lang: "ru".into(),
            route: "fast".into(),
            action: Some("info.time".into()),
            ok: true,
        }
    }

    #[tokio::test]
    async fn facts_round_trip_and_upsert_by_key() {
        let (_dir, db) = temp_db();
        let id = db
            .fact_upsert(
                "project".into(),
                "путь".into(),
                r"C:\Users\example\jarvis".into(),
            )
            .await
            .expect("upsert")
            .expect("id");
        let again = db
            .fact_upsert("project".into(), "путь".into(), r"D:\work".into())
            .await
            .expect("upsert")
            .expect("id");
        assert_eq!(id, again);
        let facts = db.fact_list(None, 10).await.expect("list");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].value, r"D:\work");
        assert_eq!(db.fact_get(id).await.expect("get"), Some(facts[0].clone()));
        assert!(db.fact_delete(id).await.expect("delete"));
        assert!(!db.fact_delete(id).await.expect("delete twice"));
        assert!(db.fact_list(None, 10).await.expect("list").is_empty());
    }

    #[tokio::test]
    async fn relevant_facts_match_cyrillic_case_and_endings() {
        let (_dir, db) = temp_db();
        for (kind, key, value) in [
            ("project", "Проект Jarvis", r"C:\Users\example\jarvis"),
            ("preference", "музыка", "джаз по вечерам"),
            ("device", "наушники", "Sony WH-1000"),
        ] {
            db.fact_upsert(kind.into(), key.into(), value.into())
                .await
                .expect("upsert");
        }
        let found = db
            .fact_relevant("где лежит мой ПРОЕКТ".into(), 10)
            .await
            .expect("relevant");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "project");
        let found = db
            .fact_relevant("какую музыку я люблю".into(), 10)
            .await
            .expect("relevant");
        assert_eq!(found[0].kind, "preference");
        assert!(db
            .fact_relevant("погода завтра".into(), 10)
            .await
            .expect("relevant")
            .is_empty());
    }

    #[tokio::test]
    async fn facts_are_capped() {
        let (_dir, db) = temp_db();
        for index in 0..MAX_FACTS {
            let stored = db
                .fact_upsert("note".into(), format!("k{index}"), "v".into())
                .await
                .expect("upsert");
            assert!(stored.is_some());
        }
        assert_eq!(
            db.fact_upsert("note".into(), "extra".into(), "v".into())
                .await
                .expect("upsert"),
            None
        );
        // An existing key can still be updated.
        assert!(db
            .fact_upsert("note".into(), "k0".into(), "new".into())
            .await
            .expect("upsert")
            .is_some());
    }

    #[tokio::test]
    async fn history_add_list_search_delete() {
        let (_dir, db) = temp_db();
        let first = db.history_add(entry("который час")).await.expect("add");
        db.history_add(entry("открой блокнот")).await.expect("add");
        let all = db.history_list(None, 10).await.expect("list");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].text, "открой блокнот");
        let found = db
            .history_list(Some("КОТОРЫЙ".into()), 10)
            .await
            .expect("search");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, first);
        assert!(db.history_delete(first).await.expect("delete"));
        assert_eq!(db.history_list(None, 10).await.expect("list").len(), 1);
        assert_eq!(db.history_clear().await.expect("clear"), 1);
    }

    #[tokio::test]
    async fn retention_purges_only_old_rows() {
        let (_dir, db) = temp_db();
        let old = db.history_add(entry("старое")).await.expect("add");
        db.history_add(entry("новое")).await.expect("add");
        db.history_backdate(old, 40).await.expect("backdate");
        assert_eq!(db.history_purge(30).await.expect("purge"), 1);
        let left = db.history_list(None, 10).await.expect("list");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].text, "новое");
    }

    #[tokio::test]
    async fn apps_and_clear_all() {
        let (_dir, db) = temp_db();
        db.app_upsert("редактор".into(), r"D:\ed.exe".into())
            .await
            .expect("app");
        db.app_upsert("редактор".into(), r"D:\ed2.exe".into())
            .await
            .expect("app again");
        let apps = db.app_list().await.expect("apps");
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].target, r"D:\ed2.exe");
        db.fact_upsert("note".into(), "a".into(), "b".into())
            .await
            .expect("fact");
        db.history_add(entry("x")).await.expect("history");
        db.memory_clear_all().await.expect("clear");
        assert!(db.app_list().await.expect("apps").is_empty());
        assert!(db.fact_list(None, 5).await.expect("facts").is_empty());
        assert!(db.history_list(None, 5).await.expect("history").is_empty());
        assert!(!db.app_delete(apps[0].id).await.expect("delete"));
    }

    #[test]
    fn word_helpers() {
        assert_eq!(words("Привет, мир! A", 2), vec!["привет", "мир"]);
        assert_eq!(relevance(&words("проекты", 2), "мой проект"), 1);
        assert_eq!(relevance(&words("кот", 2), "мой проект"), 0);
    }
}
