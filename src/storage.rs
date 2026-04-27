use rusqlite::{params, Connection, Result as SqlResult};
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub room_id: String,
    pub from_id: String,
    pub text: String,
    pub ts: i64,
}

pub struct MessageStore {
    conn: Connection,
}

impl MessageStore {
    pub fn open(path: &PathBuf) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                room_id TEXT NOT NULL,
                from_id TEXT NOT NULL,
                text TEXT NOT NULL,
                ts INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_room_ts ON messages(room_id, ts);"
        )?;
        Ok(Self { conn })
    }

    pub fn save(&self, msg: &StoredMessage) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO messages (id, room_id, from_id, text, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![msg.id, msg.room_id, msg.from_id, msg.text, msg.ts],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn load_recent(&self, room_id: &str, limit: usize) -> SqlResult<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, room_id, from_id, text, ts FROM messages
             WHERE room_id = ?1 ORDER BY ts DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![room_id, limit as i64], |row| {
            Ok(StoredMessage {
                id: row.get(0)?,
                room_id: row.get(1)?,
                from_id: row.get(2)?,
                text: row.get(3)?,
                ts: row.get(4)?,
            })
        })?;
        let mut msgs: Vec<StoredMessage> = rows.filter_map(|r| r.ok()).collect();
        msgs.reverse();
        Ok(msgs)
    }

    #[allow(dead_code)]
    pub fn load_before(&self, room_id: &str, before_ts: i64, limit: usize) -> SqlResult<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, room_id, from_id, text, ts FROM messages
             WHERE room_id = ?1 AND ts < ?2 ORDER BY ts DESC LIMIT ?3"
        )?;
        let rows = stmt.query_map(params![room_id, before_ts, limit as i64], |row| {
            Ok(StoredMessage {
                id: row.get(0)?,
                room_id: row.get(1)?,
                from_id: row.get(2)?,
                text: row.get(3)?,
                ts: row.get(4)?,
            })
        })?;
        let mut msgs: Vec<StoredMessage> = rows.filter_map(|r| r.ok()).collect();
        msgs.reverse();
        Ok(msgs)
    }

    #[allow(dead_code)]
    pub fn load_all(&self, room_id: &str) -> SqlResult<Vec<StoredMessage>> {
        self.load_before(room_id, i64::MAX, usize::MAX)
    }

    pub fn list_rooms(&self) -> SqlResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT room_id FROM messages ORDER BY MAX(ts) DESC"
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn last_ts(&self, room_id: &str) -> SqlResult<Option<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT MAX(ts) FROM messages WHERE room_id = ?1"
        )?;
        Ok(stmt.query_row(params![room_id], |row| row.get(0))?)
    }
}
