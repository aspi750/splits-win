use crate::models::*;
use chrono::Local;
use rusqlite::{params, Connection};
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Keyring error: {0}")]
    Keyring(String),
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, Default)]
pub struct AppSettings {
    pub custom_shared_coach_directory_path: Option<String>,
    pub execution_mode: ExecutionMode,
    pub uiserver_endpoint_url: Option<String>,
    pub morning_notification_enabled: bool,
    pub daily_brief_notified_date: Option<String>,
    pub daily_brief_date: Option<String>,
    pub daily_brief_text: Option<String>,
    pub daily_brief_had_sleep: bool,
    pub profile_onboarded: bool,
}

pub struct SharedStore {
    root_dir: PathBuf,
}

impl SharedStore {
    pub fn default_coach_dir() -> PathBuf {
        #[cfg(target_os = "windows")]
        let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        #[cfg(not(target_os = "windows"))]
        let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));

        base.join("SplitsCoach").join("coach")
    }

    pub fn new(override_root: Option<PathBuf>) -> Result<Self, StorageError> {
        let root_dir = override_root.unwrap_or_else(Self::default_coach_dir);
        Self::ensure_subdirectories(&root_dir)?;
        Ok(Self { root_dir })
    }

    pub fn root(&self) -> &Path {
        &self.root_dir
    }

    pub fn ensure_subdirectories(root: &Path) -> Result<(), StorageError> {
        for sub in ["", "state", "history", "attachments"] {
            let dir = if sub.is_empty() { root.to_path_buf() } else { root.join(sub) };
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn read_text(&self, filename: &str) -> Result<Option<String>, StorageError> {
        let path = self.root_dir.join(filename);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read_to_string(path)?))
    }

    pub fn write_text_atomic(&self, filename: &str, content: &str) -> Result<(), StorageError> {
        let target = self.root_dir.join(filename);
        self.backup_if_exists(&target)?;
        Self::atomic_write_bytes(&target, content.as_bytes())
    }

    pub fn read_json<T: DeserializeOwned>(&self, filename: &str) -> Result<Option<T>, StorageError> {
        let path = self.root_dir.join(filename);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(path)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    pub fn write_json_atomic<T: Serialize>(&self, filename: &str, value: &T) -> Result<(), StorageError> {
        let target = self.root_dir.join(filename);
        self.backup_if_exists(&target)?;
        let data = serde_json::to_vec_pretty(value)?;
        Self::atomic_write_bytes(&target, &data)
    }

    pub fn load_athlete_config(&self) -> Result<Option<AthleteConfig>, StorageError> {
        self.read_json("athlete_config.json")
    }

    pub fn save_athlete_config(&self, config: &AthleteConfig) -> Result<(), StorageError> {
        self.write_json_atomic("athlete_config.json", config)
    }

    pub fn append_usage_log(&self, entry: &CoachUsageEntry) -> Result<(), StorageError> {
        let line = format!("{}\n", serde_json::to_string(entry)?);
        let path = self.root_dir.join("coach_usage.log");
        let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    fn backup_if_exists(&self, target: &Path) -> Result<(), StorageError> {
        if !target.exists() {
            return Ok(());
        }
        let history_dir = self.root_dir.join("history");
        fs::create_dir_all(&history_dir)?;
        let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
        let ext = target.extension().and_then(|s| s.to_str()).unwrap_or("");
        let stamp = Local::now().format("%Y-%m-%dT%H%M%S").to_string();
        let backup_name = if ext.is_empty() {
            format!("{stem}.{stamp}")
        } else {
            format!("{stem}.{stamp}.{ext}")
        };
        let bytes = fs::read(target)?;
        Self::atomic_write_bytes(&history_dir.join(backup_name), &bytes)
    }

    pub fn atomic_write_bytes(target: &Path, data: &[u8]) -> Result<(), StorageError> {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_name = format!(
            ".{}.tmp.{}",
            target.file_name().and_then(|s| s.to_str()).unwrap_or("file"),
            uuid::Uuid::new_v4()
        );
        let tmp_path = target.with_file_name(tmp_name);
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(data)?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, target)?;
        Ok(())
    }
}

pub struct CredentialStore;

impl CredentialStore {
    pub fn save_secret(service: &str, account: &str, value: &str) -> Result<(), StorageError> {
        let entry = keyring::Entry::new(service, account).map_err(|e| StorageError::Keyring(e.to_string()))?;
        entry.set_password(value.trim()).map_err(|e| StorageError::Keyring(e.to_string()))
    }

    pub fn read_secret(service: &str, account: &str) -> Option<String> {
        let entry = keyring::Entry::new(service, account).ok()?;
        entry.get_password().ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    pub fn delete_secret(service: &str, account: &str) -> Result<(), StorageError> {
        let entry = keyring::Entry::new(service, account).map_err(|e| StorageError::Keyring(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(StorageError::Keyring(e.to_string())),
        }
    }
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), StorageError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS activities (
                garmin_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                type_key TEXT NOT NULL,
                start_iso TEXT NOT NULL,
                distance_m REAL NOT NULL,
                duration_sec REAL NOT NULL,
                moving_duration_sec REAL NOT NULL,
                elevation_gain_m REAL NOT NULL,
                average_hr REAL,
                max_hr REAL,
                cadence_spm REAL,
                training_load REAL,
                shoe_name TEXT
            );
            CREATE TABLE IF NOT EXISTS days (
                date TEXT PRIMARY KEY,
                resting_hr INTEGER,
                stress_avg INTEGER,
                body_battery_high INTEGER,
                body_battery_latest INTEGER,
                steps INTEGER,
                step_goal INTEGER,
                sleep_seconds INTEGER,
                sleep_score INTEGER,
                hrv_last_night INTEGER,
                hrv_status TEXT
            );",
        )?;
        Ok(())
    }

    pub fn upsert_activity(&self, act: &ActivityRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO activities (
                garmin_id, name, type_key, start_iso, distance_m, duration_sec, moving_duration_sec,
                elevation_gain_m, average_hr, max_hr, cadence_spm, training_load, shoe_name
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(garmin_id) DO UPDATE SET
                name=excluded.name, type_key=excluded.type_key, start_iso=excluded.start_iso,
                distance_m=excluded.distance_m, duration_sec=excluded.duration_sec,
                moving_duration_sec=excluded.moving_duration_sec, elevation_gain_m=excluded.elevation_gain_m,
                average_hr=excluded.average_hr, max_hr=excluded.max_hr, cadence_spm=excluded.cadence_spm,
                training_load=excluded.training_load, shoe_name=excluded.shoe_name",
            params![
                act.garmin_id, act.name, act.type_key, act.start_iso, act.distance_m,
                act.duration_sec, act.moving_duration_sec, act.elevation_gain_m,
                act.average_hr, act.max_hr, act.cadence_spm, act.training_load, act.shoe_name
            ],
        )?;
        Ok(())
    }

    pub fn list_activities(&self, limit: usize) -> Result<Vec<ActivityRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT garmin_id, name, type_key, start_iso, distance_m, duration_sec, moving_duration_sec,
                    elevation_gain_m, average_hr, max_hr, cadence_spm, training_load, shoe_name
             FROM activities ORDER BY start_iso DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(ActivityRecord {
                garmin_id: r.get(0)?,
                name: r.get(1)?,
                type_key: r.get(2)?,
                start_iso: r.get(3)?,
                distance_m: r.get(4)?,
                duration_sec: r.get(5)?,
                moving_duration_sec: r.get(6)?,
                elevation_gain_m: r.get(7)?,
                average_hr: r.get(8)?,
                max_hr: r.get(9)?,
                cadence_spm: r.get(10)?,
                training_load: r.get(11)?,
                shoe_name: r.get(12)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn upsert_day(&self, day: &DayRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO days (
                date, resting_hr, stress_avg, body_battery_high, body_battery_latest,
                steps, step_goal, sleep_seconds, sleep_score, hrv_last_night, hrv_status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(date) DO UPDATE SET
                resting_hr=COALESCE(excluded.resting_hr, days.resting_hr),
                stress_avg=COALESCE(excluded.stress_avg, days.stress_avg),
                body_battery_high=COALESCE(excluded.body_battery_high, days.body_battery_high),
                body_battery_latest=COALESCE(excluded.body_battery_latest, days.body_battery_latest),
                steps=COALESCE(excluded.steps, days.steps),
                step_goal=COALESCE(excluded.step_goal, days.step_goal),
                sleep_seconds=COALESCE(excluded.sleep_seconds, days.sleep_seconds),
                sleep_score=COALESCE(excluded.sleep_score, days.sleep_score),
                hrv_last_night=COALESCE(excluded.hrv_last_night, days.hrv_last_night),
                hrv_status=COALESCE(excluded.hrv_status, days.hrv_status)",
            params![
                day.date, day.resting_hr, day.stress_avg, day.body_battery_high,
                day.body_battery_latest, day.steps, day.step_goal, day.sleep_seconds,
                day.sleep_score, day.hrv_last_night, day.hrv_status
            ],
        )?;
        Ok(())
    }

    pub fn list_days(&self, limit: usize) -> Result<Vec<DayRecord>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT date, resting_hr, stress_avg, body_battery_high, body_battery_latest,
                    steps, step_goal, sleep_seconds, sleep_score, hrv_last_night, hrv_status
             FROM days ORDER BY date DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(DayRecord {
                date: r.get(0)?,
                resting_hr: r.get(1)?,
                stress_avg: r.get(2)?,
                body_battery_high: r.get(3)?,
                body_battery_latest: r.get(4)?,
                steps: r.get(5)?,
                step_goal: r.get(6)?,
                sleep_seconds: r.get(7)?,
                sleep_score: r.get(8)?,
                hrv_last_night: r.get(9)?,
                hrv_status: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}
