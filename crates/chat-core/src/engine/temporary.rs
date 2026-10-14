//! Temporary mode: conversations work normally, but nothing about them is kept on this
//! device after the app closes.
//!
//! The content tables are shadowed by SQLite TEMP tables (temp_store = MEMORY, so they live
//! in RAM only): unqualified queries hit the temp copy first, so the rest of the engine is
//! unchanged. Media of messages go to a separate folder that is wiped at startup and exit.
//! Nothing of it is backed up to the vault. Group conversation rows stay in the main database
//! (membership is server state anyway), their messages do not.

use std::path::PathBuf;

use super::Inner;
use crate::error::Result;

/// Tables holding conversation content.
const TABLES: &[&str] = &["conversations", "messages", "reactions", "receipts", "outbox", "message_marks"];

impl Inner {
    pub fn temporary_mode(&self) -> bool {
        self.db.borrow().kv_get_json::<bool>("temporary_mode").ok().flatten().unwrap_or(false)
    }

    pub fn temp_media_dir(&self) -> PathBuf {
        self.data_dir.join("media-temp")
    }

    /// Where message attachments are written.
    pub fn attachment_dir(&self) -> PathBuf {
        if self.temporary_mode() {
            self.temp_media_dir()
        } else {
            self.media_dir()
        }
    }

    /// Deletes the media of temporary conversations (startup, exit, mode change).
    pub fn wipe_temp_media(&self) {
        let dir = self.temp_media_dir();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
    }

    fn create_temp_tables(&self) -> Result<()> {
        let db = self.db.borrow();
        for table in TABLES {
            let exists: i64 =
                db.conn.query_row("SELECT COUNT(*) FROM sqlite_temp_master WHERE type = 'table' AND name = ?1", [table], |r| r.get(0))?;
            if exists > 0 {
                continue;
            }
            let sql: String = db.conn.query_row("SELECT sql FROM main.sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| r.get(0))?;
            db.conn.execute_batch(&sql.replacen("CREATE TABLE", "CREATE TEMP TABLE", 1))?;
            let mut st = db.conn.prepare("SELECT name, sql FROM main.sqlite_master WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL")?;
            let indexes: Vec<(String, String)> = st.query_map([table], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            for (name, sql) in indexes {
                db.conn.execute_batch(&sql.replacen(&format!("INDEX {name}"), &format!("INDEX temp.tmp_{name}"), 1))?;
            }
        }
        Ok(())
    }

    /// Moves what is stored into memory; group conversation rows stay stored too.
    fn move_content_to_memory(&self) -> Result<()> {
        let db = self.db.borrow();
        for table in TABLES {
            db.conn.execute(&format!("INSERT OR REPLACE INTO temp.{table} SELECT * FROM main.{table}"), [])?;
            let only_direct = if *table == "conversations" { " WHERE group_id IS NULL" } else { "" };
            db.conn.execute(&format!("DELETE FROM main.{table}{only_direct}"), [])?;
        }
        Ok(())
    }

    /// Called at startup: in temporary mode, start from an empty memory copy.
    pub(super) fn start_temporary_mode(&self) -> Result<()> {
        if !self.temporary_mode() {
            return Ok(());
        }
        self.wipe_temp_media();
        self.create_temp_tables()?;
        // Anything content-like left on disk (e.g. after a crash mid-switch) goes.
        self.move_content_to_memory()?;
        let db = self.db.borrow();
        for table in TABLES {
            if *table != "conversations" {
                db.conn.execute(&format!("DELETE FROM temp.{table}"), [])?;
            }
        }
        db.conn.execute("DELETE FROM temp.conversations WHERE group_id IS NULL", [])?;
        Ok(())
    }

    /// Turns temporary mode on (current history moves into memory and is wiped from disk) or
    /// off (the current session is saved normally from now on).
    pub fn set_temporary_mode(&self, on: bool) -> Result<()> {
        if on == self.temporary_mode() {
            return Ok(());
        }
        if on {
            self.create_temp_tables()?;
            self.move_content_to_memory()?;
            self.move_media(&self.media_dir(), &self.temp_media_dir())?;
            self.db.borrow().kv_set_json("temporary_mode", &true)?;
            self.compact_database()?;
        } else {
            self.db.borrow().kv_set_json("temporary_mode", &false)?;
            self.move_media(&self.temp_media_dir(), &self.media_dir())?;
            let db = self.db.borrow();
            for table in TABLES {
                db.conn.execute(&format!("INSERT OR REPLACE INTO main.{table} SELECT * FROM temp.{table}"), [])?;
                db.conn.execute_batch(&format!("DROP TABLE temp.{table};"))?;
            }
        }
        self.emit("conversations_reset", &serde_json::Value::Null);
        Ok(())
    }

    /// Moves message attachments between the stored and the temporary media folder and
    /// rewrites their paths (avatars stay).
    fn move_media(&self, from: &PathBuf, to: &PathBuf) -> Result<()> {
        std::fs::create_dir_all(to)?;
        if let Ok(entries) = std::fs::read_dir(from) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with("avatar-") {
                    let _ = std::fs::rename(e.path(), to.join(&name));
                }
            }
        }
        let (from_s, to_s) = (from.to_string_lossy().to_string(), to.to_string_lossy().to_string());
        // JSON-escaped forms too (backslashes on Windows).
        let (from_j, to_j) = (serde_json::to_string(&from_s)?, serde_json::to_string(&to_s)?);
        let db = self.db.borrow();
        db.conn.execute("UPDATE messages SET attachments_json = REPLACE(attachments_json, ?1, ?2)", [&from_s, &to_s])?;
        db.conn.execute(
            "UPDATE messages SET attachments_json = REPLACE(attachments_json, ?1, ?2)",
            [from_j.trim_matches('"'), to_j.trim_matches('"')],
        )?;
        Ok(())
    }
}
