use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::types::SignalValue;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SignalSample {
    pub timestamp_ms: i64,
    pub direction: String,
    pub signal: String,
    pub value: SignalValue,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CycleMetric {
    pub timestamp_ms: i64,
    pub duration_us: u64,
    pub jitter_us: i64,
    pub deadline_miss: bool,
}

#[derive(Debug, Clone)]
pub struct SqliteStorage {
    db_path: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStorage {
    pub fn new(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        let conn = Connection::open(&db_path)?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS signal_samples (
                id INTEGER PRIMARY KEY,
                timestamp_ms INTEGER NOT NULL,
                direction TEXT NOT NULL,
                signal TEXT NOT NULL,
                value_f64 REAL,
                value_bool INTEGER
            );

            CREATE TABLE IF NOT EXISTS cycle_metrics (
                id INTEGER PRIMARY KEY,
                timestamp_ms INTEGER NOT NULL,
                duration_us INTEGER NOT NULL,
                jitter_us INTEGER NOT NULL,
                deadline_miss INTEGER NOT NULL
            );
            ",
        )?;

        Ok(Self {
            db_path,
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn store_signal(&self, direction: &str, signal: &str, value: &SignalValue) -> Result<()> {
        let now = now_ms();
        let (v_f64, v_bool): (Option<f64>, Option<i64>) = match value {
            SignalValue::F64(v) => (Some(*v), None),
            SignalValue::Bool(v) => (None, Some(i64::from(*v))),
        };

        self.conn.lock().execute(
            "INSERT INTO signal_samples(timestamp_ms, direction, signal, value_f64, value_bool)
            VALUES (?1, ?2, ?3, ?4, ?5)",
            params![now, direction, signal, v_f64, v_bool],
        )?;
        Ok(())
    }

    pub fn store_signal_map(
        &self,
        direction: &str,
        values: &HashMap<String, SignalValue>,
    ) -> Result<()> {
        for (signal, value) in values {
            self.store_signal(direction, signal, value)?;
        }
        Ok(())
    }

    pub fn read_recent_samples(&self, limit: usize) -> Result<Vec<SignalSample>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT timestamp_ms, direction, signal, value_f64, value_bool
             FROM signal_samples
             ORDER BY id DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map([limit as i64], |row| {
            let timestamp_ms: i64 = row.get(0)?;
            let direction: String = row.get(1)?;
            let signal: String = row.get(2)?;
            let value_f64: Option<f64> = row.get(3)?;
            let value_bool: Option<i64> = row.get(4)?;
            let value = match (value_f64, value_bool) {
                (Some(v), _) => SignalValue::F64(v),
                (None, Some(v)) => SignalValue::Bool(v != 0),
                (None, None) => SignalValue::F64(0.0),
            };
            Ok(SignalSample {
                timestamp_ms,
                direction,
                signal,
                value,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        out.reverse();
        Ok(out)
    }

    pub fn store_cycle_metric(&self, metric: &CycleMetric) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO cycle_metrics(timestamp_ms, duration_us, jitter_us, deadline_miss)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                metric.timestamp_ms,
                metric.duration_us as i64,
                metric.jitter_us,
                i64::from(metric.deadline_miss)
            ],
        )?;
        Ok(())
    }

    pub fn read_recent_cycle_metrics(&self, limit: usize) -> Result<Vec<CycleMetric>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT timestamp_ms, duration_us, jitter_us, deadline_miss
             FROM cycle_metrics
             ORDER BY id DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map([limit as i64], |row| {
            Ok(CycleMetric {
                timestamp_ms: row.get(0)?,
                duration_us: row.get::<_, i64>(1)? as u64,
                jitter_us: row.get(2)?,
                deadline_miss: row.get::<_, i64>(3)? != 0,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        out.reverse();
        Ok(out)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn stores_and_reads_signal_samples() {
        let dir = tempdir().expect("tmp dir");
        let db = SqliteStorage::new(dir.path().join("samples.sqlite")).expect("create db");

        db.store_signal("input", "pv", &SignalValue::F64(10.5))
            .expect("store input");
        db.store_signal("output", "mv", &SignalValue::Bool(true))
            .expect("store output");

        let rows = db.read_recent_samples(10).expect("read rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].signal, "pv");
        assert_eq!(rows[1].signal, "mv");
    }
}
