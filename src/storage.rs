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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleValueKind {
    F64,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SeriesCatalogEntry {
    pub direction: String,
    pub signal: String,
    pub value_kind: SampleValueKind,
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
            CREATE INDEX IF NOT EXISTS idx_signal_samples_timestamp
                ON signal_samples(timestamp_ms);
            CREATE INDEX IF NOT EXISTS idx_signal_samples_direction_signal_timestamp
                ON signal_samples(direction, signal, timestamp_ms);

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
        self.store_signal_at(now_ms(), direction, signal, value)
    }

    pub fn store_signal_at(
        &self,
        timestamp_ms: i64,
        direction: &str,
        signal: &str,
        value: &SignalValue,
    ) -> Result<()> {
        let (v_f64, v_bool): (Option<f64>, Option<i64>) = match value {
            SignalValue::F64(v) => (Some(*v), None),
            SignalValue::Bool(v) => (None, Some(i64::from(*v))),
        };

        self.conn.lock().execute(
            "INSERT INTO signal_samples(timestamp_ms, direction, signal, value_f64, value_bool)
            VALUES (?1, ?2, ?3, ?4, ?5)",
            params![timestamp_ms, direction, signal, v_f64, v_bool],
        )?;
        Ok(())
    }

    pub fn store_signal_map(
        &self,
        direction: &str,
        values: &HashMap<String, SignalValue>,
    ) -> Result<()> {
        self.store_signal_map_at(now_ms(), direction, values)
    }

    pub fn store_signal_map_at(
        &self,
        timestamp_ms: i64,
        direction: &str,
        values: &HashMap<String, SignalValue>,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO signal_samples(timestamp_ms, direction, signal, value_f64, value_bool)
                VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (signal, value) in values {
                let (v_f64, v_bool): (Option<f64>, Option<i64>) = match value {
                    SignalValue::F64(v) => (Some(*v), None),
                    SignalValue::Bool(v) => (None, Some(i64::from(*v))),
                };
                stmt.execute(params![timestamp_ms, direction, signal, v_f64, v_bool])?;
            }
        }
        tx.commit()?;
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

    pub fn read_samples_since(&self, since_timestamp_ms: i64) -> Result<Vec<SignalSample>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT timestamp_ms, direction, signal, value_f64, value_bool
             FROM signal_samples
             WHERE timestamp_ms >= ?1
             ORDER BY timestamp_ms ASC, id ASC",
        )?;

        let rows = stmt.query_map([since_timestamp_ms], |row| {
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
        Ok(out)
    }

    pub fn read_latest_samples_window(&self, window_ms: u64) -> Result<Vec<SignalSample>> {
        let conn = self.conn.lock();
        let latest_timestamp_ms: Option<i64> =
            conn.query_row("SELECT MAX(timestamp_ms) FROM signal_samples", [], |row| {
                row.get(0)
            })?;
        drop(conn);

        let Some(latest_timestamp_ms) = latest_timestamp_ms else {
            return Ok(Vec::new());
        };
        let since = latest_timestamp_ms.saturating_sub(window_ms as i64);
        self.read_samples_since(since)
    }

    pub fn read_series_catalog(&self) -> Result<Vec<SeriesCatalogEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT direction, signal,
                    CASE WHEN value_f64 IS NOT NULL THEN 'f64' ELSE 'bool' END AS value_kind
             FROM signal_samples
             GROUP BY direction, signal, value_kind
             ORDER BY direction ASC, signal ASC, value_kind ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            let value_kind = match row.get::<_, String>(2)?.as_str() {
                "f64" => SampleValueKind::F64,
                _ => SampleValueKind::Bool,
            };
            Ok(SeriesCatalogEntry {
                direction: row.get(0)?,
                signal: row.get(1)?,
                value_kind,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
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

    #[test]
    fn reads_samples_since_timestamp() {
        let dir = tempdir().expect("tmp dir");
        let db = SqliteStorage::new(dir.path().join("samples.sqlite")).expect("create db");

        db.store_signal("input", "cv.y1.setpoint", &SignalValue::F64(1.0))
            .expect("store first");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let cutoff = now_ms();
        std::thread::sleep(std::time::Duration::from_millis(2));
        db.store_signal("output", "mv.u1.write", &SignalValue::F64(2.0))
            .expect("store second");

        let rows = db.read_samples_since(cutoff).expect("read window");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].direction, "output");
        assert_eq!(rows[0].signal, "mv.u1.write");
    }

    #[test]
    fn reads_series_catalog() {
        let dir = tempdir().expect("tmp dir");
        let db = SqliteStorage::new(dir.path().join("samples.sqlite")).expect("create db");

        db.store_signal("input", "cv.y1.process_value", &SignalValue::F64(1.0))
            .expect("store input");
        db.store_signal("online_param", "p.y1.u1.kp", &SignalValue::F64(2.0))
            .expect("store param");
        db.store_signal("user_log", "ff.enable", &SignalValue::Bool(true))
            .expect("store log");

        let catalog = db.read_series_catalog().expect("catalog");
        assert!(catalog.contains(&SeriesCatalogEntry {
            direction: "input".to_string(),
            signal: "cv.y1.process_value".to_string(),
            value_kind: SampleValueKind::F64,
        }));
        assert!(catalog.contains(&SeriesCatalogEntry {
            direction: "online_param".to_string(),
            signal: "p.y1.u1.kp".to_string(),
            value_kind: SampleValueKind::F64,
        }));
        assert!(catalog.contains(&SeriesCatalogEntry {
            direction: "user_log".to_string(),
            signal: "ff.enable".to_string(),
            value_kind: SampleValueKind::Bool,
        }));
    }
}
