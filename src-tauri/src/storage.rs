use crate::models::*;
use chrono::Local;
use rusqlite::{params, Connection};
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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

    pub fn load_first_use_state(&self) -> Result<FirstUseFlowState, StorageError> {
        if let Some(existing) = self.read_json::<FirstUseFlowState>("state/first_use_flow.json")? {
            return Ok(existing);
        }
        let has_profile = self
            .read_text("COACH_PROFILE.md")?
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let has_plan = self
            .read_text("TRAINING_PLAN.md")?
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let mut state = FirstUseFlowState::default();
        if has_profile && has_plan {
            state.stage = FirstUseStage::Active;
        }
        Ok(state)
    }

    pub fn save_first_use_state(&self, state: &FirstUseFlowState) -> Result<(), StorageError> {
        let mut next = state.clone();
        next.updated_at = chrono::Utc::now().to_rfc3339();
        let target = self.root_dir.join("state/first_use_flow.json");
        let data = serde_json::to_vec_pretty(&next)?;
        Self::atomic_write_bytes(&target, &data)
    }

    pub fn remove_relative_file(&self, rel: &str) -> Result<(), StorageError> {
        let trimmed = rel.trim().trim_start_matches(['/', '\\']);
        if trimmed.is_empty() || trimmed.contains("..") {
            return Ok(());
        }
        let path = self.root_dir.join(trimmed);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn clean_slate_files(&self) -> Result<(), StorageError> {
        for rel in [
            "athlete_config.json",
            "COACH_PROFILE.md",
            "TRAINING_PLAN.md",
            "plan.json",
            "readiness_state.json",
            "today_adapted_target.json",
            "gear_inventory.json",
            "outcomes.json",
            "coach_usage.log",
            "ASPI_Training_Log.txt",
            "state/first_use_flow.json",
        ] {
            let p = self.root_dir.join(rel);
            if p.exists() {
                let _ = fs::remove_file(&p);
            }
        }
        for sub in ["attachments", "history"] {
            let dir = self.root_dir.join(sub);
            if dir.exists() {
                let _ = fs::remove_dir_all(&dir);
            }
        }
        Self::ensure_subdirectories(&self.root_dir)?;
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

    pub fn clear_all() -> Result<(), StorageError> {
        let _ = Self::delete_secret(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT);
        let _ = Self::delete_secret(APP_SERVICE, GARMIN_ACCOUNT);
        let _ = Self::delete_secret(APP_SERVICE, GEMINI_ACCOUNT);
        let _ = Self::delete_secret(APP_SERVICE, UISERVER_ACCOUNT);
        Ok(())
    }
}

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = Self { conn: Mutex::new(conn) };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn: Mutex::new(conn) };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), StorageError> {
        self.conn.lock().unwrap().execute_batch(
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
            );
            CREATE TABLE IF NOT EXISTS fit_tracks (
                garmin_id INTEGER PRIMARY KEY,
                track_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS user_profile (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                athlete_type TEXT NOT NULL DEFAULT 'runner',
                onboarding_completed INTEGER NOT NULL DEFAULT 0,
                runner_config_json TEXT NOT NULL DEFAULT '{}',
                hyrox_config_json TEXT NOT NULL DEFAULT '{}'
            );
            CREATE TABLE IF NOT EXISTS weight_entries (
                timestamp_epoch_sec INTEGER PRIMARY KEY,
                timestamp_iso TEXT NOT NULL,
                weight_kg REAL NOT NULL,
                body_fat_pct REAL,
                body_water_pct REAL,
                muscle_mass_kg REAL,
                source TEXT NOT NULL DEFAULT 'garmin_scale'
            );
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;
        let conn = self.conn.lock().unwrap();
        // Backwards-compatible column additions on existing databases
        let _ = conn.execute("ALTER TABLE activities ADD COLUMN is_hyrox INTEGER", []);
        let _ = conn.execute("ALTER TABLE activities ADD COLUMN post_run_context_json TEXT", []);
        let _ = conn.execute("ALTER TABLE activities ADD COLUMN fit_temp_source TEXT", []);
        let _ = conn.execute("ALTER TABLE activities ADD COLUMN garmin_notes TEXT", []);

        let _ = conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES ('global_model_id', ?1)",
            params![DEFAULT_GEMINI_MODEL_ID],
        );
        let _ = conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES ('global_thinking_level', ?1)",
            params![DEFAULT_GEMINI_THINKING_LEVEL],
        );

        let has_profile: i64 = conn.query_row(
            "SELECT COUNT(*) FROM user_profile WHERE id = 1",
            [],
            |r| r.get(0),
        )?;
        if has_profile == 0 {
            let act_count: i64 = conn.query_row("SELECT COUNT(*) FROM activities", [], |r| r.get(0))?;
            let onboarding = if act_count > 0 { 1 } else { 0 };
            conn.execute(
                "INSERT INTO user_profile (id, athlete_type, onboarding_completed, runner_config_json, hyrox_config_json)
                 VALUES (1, 'runner', ?1, '{}', '{}')",
                params![onboarding],
            )?;
        }
        Ok(())
    }

    pub fn get_user_profile(&self) -> Result<UserProfile, StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT athlete_type, onboarding_completed, runner_config_json, hyrox_config_json
             FROM user_profile WHERE id = 1 LIMIT 1",
            [],
            |r| {
                let type_str: String = r.get(0)?;
                let onboarded: i64 = r.get(1)?;
                let runner_json: String = r.get(2)?;
                let hyrox_json: String = r.get(3)?;
                Ok(UserProfile {
                    athlete_type: AthleteType::from_str_lossy(&type_str),
                    onboarding_completed: onboarded != 0,
                    runner_config: serde_json::from_str(&runner_json).unwrap_or_default(),
                    hyrox_config: serde_json::from_str(&hyrox_json).unwrap_or_default(),
                })
            },
        )
        .map_err(StorageError::from)
    }

    pub fn save_user_profile(&self, profile: &UserProfile) -> Result<(), StorageError> {
        let runner_json = serde_json::to_string(&profile.runner_config)?;
        let hyrox_json = serde_json::to_string(&profile.hyrox_config)?;
        self.conn.lock().unwrap().execute(
            "INSERT INTO user_profile (id, athlete_type, onboarding_completed, runner_config_json, hyrox_config_json)
             VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                athlete_type = excluded.athlete_type,
                onboarding_completed = excluded.onboarding_completed,
                runner_config_json = excluded.runner_config_json,
                hyrox_config_json = excluded.hyrox_config_json",
            params![
                profile.athlete_type.as_str(),
                if profile.onboarding_completed { 1 } else { 0 },
                runner_json,
                hyrox_json
            ],
        )?;
        Ok(())
    }

    pub fn set_activity_hyrox(&self, garmin_id: i64, is_hyrox: bool) -> Result<(), StorageError> {
        self.conn.lock().unwrap().execute(
            "UPDATE activities SET is_hyrox = ?1 WHERE garmin_id = ?2",
            params![if is_hyrox { 1 } else { 0 }, garmin_id],
        )?;
        Ok(())
    }

    pub fn save_post_run_context(&self, garmin_id: i64, ctx: &PostRunContext) -> Result<(), StorageError> {
        let json = serde_json::to_string(ctx)?;
        self.conn.lock().unwrap().execute(
            "UPDATE activities SET post_run_context_json = ?1 WHERE garmin_id = ?2",
            params![json, garmin_id],
        )?;
        Ok(())
    }

    pub fn set_activity_fit_temp_source(&self, garmin_id: i64, temp_source: &str) -> Result<(), StorageError> {
        self.conn.lock().unwrap().execute(
            "UPDATE activities SET fit_temp_source = ?1 WHERE garmin_id = ?2",
            params![temp_source, garmin_id],
        )?;
        Ok(())
    }

    pub fn clean_slate_reset(&self) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "DELETE FROM activities;
             DELETE FROM days;
             DELETE FROM fit_tracks;
             DELETE FROM weight_entries;
             DELETE FROM settings;",
        )?;
        conn.execute(
            "INSERT INTO user_profile (id, athlete_type, onboarding_completed, runner_config_json, hyrox_config_json)
             VALUES (1, 'runner', 0, '{}', '{}')
             ON CONFLICT(id) DO UPDATE SET
                athlete_type = 'runner',
                onboarding_completed = 0,
                runner_config_json = '{}',
                hyrox_config_json = '{}'",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES ('global_model_id', ?1)",
            params![DEFAULT_GEMINI_MODEL_ID],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES ('global_thinking_level', ?1)",
            params![DEFAULT_GEMINI_THINKING_LEVEL],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1 LIMIT 1")?;
        let mut rows = stmt.query_map(params![key], |r| r.get::<_, String>(0))?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn set_setting(&self, key: &str, value: Option<&str>) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        match value.map(str::trim).filter(|s| !s.is_empty()) {
            Some(val) => {
                conn.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, val],
                )?;
            }
            None => {
                conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
            }
        }
        Ok(())
    }

    pub fn load_model_settings(&self) -> Result<ModelSettings, StorageError> {
        let global_model_id = self
            .get_setting("global_model_id")?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_GEMINI_MODEL_ID.to_string());
        let global_thinking_level = self
            .get_setting("global_thinking_level")?
            .and_then(|s| ThinkingLevel::from_str_lossy(&s))
            .unwrap_or(ThinkingLevel::Medium);
        let coach_model_override_id = self
            .get_setting("coach_model_override_id")?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let coach_thinking_override = self
            .get_setting("coach_thinking_override")?
            .and_then(|s| ThinkingLevel::from_str_lossy(&s));
        Ok(ModelSettings {
            global_model_id,
            global_thinking_level,
            coach_model_override_id,
            coach_thinking_override,
        })
    }

    pub fn save_global_model_settings(
        &self,
        model_id: &str,
        thinking_level: ThinkingLevel,
    ) -> Result<ModelSettings, StorageError> {
        let trimmed = model_id.trim();
        let resolved_id = if trimmed.is_empty() {
            DEFAULT_GEMINI_MODEL_ID
        } else {
            trimmed
        };
        self.set_setting("global_model_id", Some(resolved_id))?;
        self.set_setting("global_thinking_level", Some(thinking_level.as_str()))?;
        self.load_model_settings()
    }

    pub fn save_coach_model_override(
        &self,
        model_id: Option<&str>,
        thinking_level: Option<ThinkingLevel>,
    ) -> Result<ModelSettings, StorageError> {
        let cleaned_id = model_id.map(str::trim).filter(|s| !s.is_empty());
        self.set_setting("coach_model_override_id", cleaned_id)?;
        self.set_setting(
            "coach_thinking_override",
            thinking_level.map(|t| t.as_str()),
        )?;
        self.load_model_settings()
    }

    pub fn update_activity_metadata(
        &self,
        garmin_id: i64,
        name: Option<&str>,
        garmin_notes: Option<&str>,
    ) -> Result<bool, StorageError> {
        let cleaned_name = name.map(str::trim).filter(|s| !s.is_empty());
        let cleaned_notes = garmin_notes.map(str::trim).filter(|s| !s.is_empty());
        let conn = self.conn.lock().unwrap();
        let changed = match cleaned_name {
            Some(n) => conn.execute(
                "UPDATE activities SET name = ?1, garmin_notes = ?2 WHERE garmin_id = ?3",
                params![n, cleaned_notes, garmin_id],
            )?,
            None => conn.execute(
                "UPDATE activities SET garmin_notes = ?1 WHERE garmin_id = ?2",
                params![cleaned_notes, garmin_id],
            )?,
        };
        Ok(changed > 0)
    }

    pub fn upsert_weight_entry(&self, entry: &WeightEntry) -> Result<(), StorageError> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO weight_entries (
                timestamp_epoch_sec, timestamp_iso, weight_kg, body_fat_pct, body_water_pct, muscle_mass_kg, source
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(timestamp_epoch_sec) DO UPDATE SET
                timestamp_iso = excluded.timestamp_iso,
                weight_kg = excluded.weight_kg,
                body_fat_pct = COALESCE(excluded.body_fat_pct, weight_entries.body_fat_pct),
                body_water_pct = COALESCE(excluded.body_water_pct, weight_entries.body_water_pct),
                muscle_mass_kg = COALESCE(excluded.muscle_mass_kg, weight_entries.muscle_mass_kg),
                source = excluded.source",
            params![
                entry.timestamp_epoch_sec,
                entry.timestamp_iso,
                entry.weight_kg,
                entry.body_fat_pct,
                entry.body_water_pct,
                entry.muscle_mass_kg,
                entry.source
            ],
        )?;
        Ok(())
    }

    pub fn list_weight_entries(&self, limit: usize) -> Result<Vec<WeightEntry>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT timestamp_epoch_sec, timestamp_iso, weight_kg, body_fat_pct, body_water_pct, muscle_mass_kg, source
             FROM weight_entries ORDER BY timestamp_epoch_sec DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(WeightEntry {
                timestamp_epoch_sec: r.get(0)?,
                timestamp_iso: r.get(1)?,
                weight_kg: r.get(2)?,
                body_fat_pct: r.get(3)?,
                body_water_pct: r.get(4)?,
                muscle_mass_kg: r.get(5)?,
                source: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn upsert_activity(&self, act: &ActivityRecord) -> Result<(), StorageError> {
        let hyrox_int = act.is_hyrox.map(|b| if b { 1i64 } else { 0i64 });
        let post_run_json = act
            .post_run_context
            .as_ref()
            .and_then(|c| serde_json::to_string(c).ok());
        let cleaned_notes = act.normalized_notes();
        self.conn.lock().unwrap().execute(
            "INSERT INTO activities (
                garmin_id, name, type_key, start_iso, distance_m, duration_sec, moving_duration_sec,
                elevation_gain_m, average_hr, max_hr, cadence_spm, training_load, shoe_name, is_hyrox, post_run_context_json, fit_temp_source, garmin_notes
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            ON CONFLICT(garmin_id) DO UPDATE SET
                name=excluded.name, type_key=excluded.type_key, start_iso=excluded.start_iso,
                distance_m=excluded.distance_m, duration_sec=excluded.duration_sec,
                moving_duration_sec=excluded.moving_duration_sec, elevation_gain_m=excluded.elevation_gain_m,
                average_hr=excluded.average_hr, max_hr=excluded.max_hr, cadence_spm=excluded.cadence_spm,
                training_load=excluded.training_load, shoe_name=excluded.shoe_name,
                is_hyrox=COALESCE(excluded.is_hyrox, activities.is_hyrox),
                post_run_context_json=COALESCE(excluded.post_run_context_json, activities.post_run_context_json),
                fit_temp_source=COALESCE(excluded.fit_temp_source, activities.fit_temp_source),
                garmin_notes=COALESCE(excluded.garmin_notes, activities.garmin_notes)",
            params![
                act.garmin_id, act.name, act.type_key, act.start_iso, act.distance_m,
                act.duration_sec, act.moving_duration_sec, act.elevation_gain_m,
                act.average_hr, act.max_hr, act.cadence_spm, act.training_load, act.shoe_name,
                hyrox_int, post_run_json, act.fit_temp_source, cleaned_notes
            ],
        )?;
        Ok(())
    }

    pub fn list_activities(&self, limit: usize) -> Result<Vec<ActivityRecord>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT garmin_id, name, type_key, start_iso, distance_m, duration_sec, moving_duration_sec,
                    elevation_gain_m, average_hr, max_hr, cadence_spm, training_load, shoe_name, is_hyrox, post_run_context_json, fit_temp_source, garmin_notes
             FROM activities ORDER BY start_iso DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            let hyrox_opt: Option<i64> = r.get(13)?;
            let post_run_str: Option<String> = r.get(14)?;
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
                is_hyrox: hyrox_opt.map(|v| v != 0),
                post_run_context: post_run_str.and_then(|s| serde_json::from_str(&s).ok()),
                fit_temp_source: r.get(15)?,
                garmin_notes: r.get(16)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn upsert_day(&self, day: &DayRecord) -> Result<(), StorageError> {
        self.conn.lock().unwrap().execute(
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
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

    pub fn get_activity(&self, garmin_id: i64) -> Result<Option<ActivityRecord>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT garmin_id, name, type_key, start_iso, distance_m, duration_sec, moving_duration_sec,
                    elevation_gain_m, average_hr, max_hr, cadence_spm, training_load, shoe_name, is_hyrox, post_run_context_json, fit_temp_source, garmin_notes
             FROM activities WHERE garmin_id = ?1 LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![garmin_id], |r| {
            let hyrox_opt: Option<i64> = r.get(13)?;
            let post_run_str: Option<String> = r.get(14)?;
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
                is_hyrox: hyrox_opt.map(|v| v != 0),
                post_run_context: post_run_str.and_then(|s| serde_json::from_str(&s).ok()),
                fit_temp_source: r.get(15)?,
                garmin_notes: r.get(16)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn upsert_fit_track(&self, garmin_id: i64, track: &FitTrackAnalysis) -> Result<(), StorageError> {
        let json = serde_json::to_string(track)?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        self.conn.lock().unwrap().execute(
            "INSERT INTO fit_tracks (garmin_id, track_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(garmin_id) DO UPDATE SET
                track_json = excluded.track_json,
                updated_at = excluded.updated_at",
            params![garmin_id, json, updated_at],
        )?;
        Ok(())
    }

    pub fn get_fit_track(&self, garmin_id: i64) -> Result<Option<FitTrackAnalysis>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT track_json FROM fit_tracks WHERE garmin_id = ?1 LIMIT 1")?;
        let mut rows = stmt.query_map(params![garmin_id], |r| r.get::<_, String>(0))?;
        match rows.next() {
            Some(raw) => {
                let text = raw?;
                let parsed: FitTrackAnalysis = serde_json::from_str(&text)?;
                Ok(Some(parsed))
            }
            None => Ok(None),
        }
    }
}
