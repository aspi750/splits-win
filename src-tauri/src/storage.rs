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
    pub fn default_base_dir() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("SplitsCoach")
    }

    pub fn default_coach_dir() -> PathBuf {
        Self::default_base_dir().join("coach")
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
            "training_ledger.jsonl",
            "state/first_use_flow.json",
        ] {
            let p = self.root_dir.join(rel);
            if p.exists() {
                let _ = fs::remove_file(&p);
            }
        }
        if let Ok(entries) = fs::read_dir(&self.root_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let remove = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map_or(false, |ext| {
                            matches!(
                                ext.to_ascii_lowercase().as_str(),
                                "fit" | "zip" | "md" | "json" | "txt" | "log"
                            )
                        });
                    if remove {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
        let state_dir = self.root_dir.join("state");
        if let Ok(entries) = fs::read_dir(&state_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let remove = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map_or(false, |ext| {
                            matches!(
                                ext.to_ascii_lowercase().as_str(),
                                "fit" | "md" | "json" | "txt" | "log"
                            )
                        });
                    if remove {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
        for sub in ["attachments", "history", ".backups", "Activities", "CoachCache"] {
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

#[derive(Debug, Clone, Serialize, serde::Deserialize, Default, PartialEq, Eq)]
pub struct CredentialsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub garmin_personal_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uiserver_auth_token: Option<String>,
}

static CREDENTIALS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Copy)]
enum CredentialField {
    Garmin,
    Gemini,
    UIServer,
}

impl CredentialField {
    fn from_account(account: &str) -> Self {
        match account {
            GARMIN_ACCOUNT => CredentialField::Garmin,
            GEMINI_ACCOUNT => CredentialField::Gemini,
            UISERVER_ACCOUNT => CredentialField::UIServer,
            _ => CredentialField::Gemini,
        }
    }
}

pub struct CredentialStore;

impl CredentialStore {
    pub fn default_credentials_path() -> PathBuf {
        SharedStore::default_base_dir().join("credentials.json")
    }

    fn read_file_unlocked(path: &Path) -> CredentialsFile {
        let Ok(bytes) = fs::read(path) else {
            return CredentialsFile::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    fn write_file_unlocked(path: &Path, creds: &CredentialsFile) -> Result<(), StorageError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_vec_pretty(creds)?;
        let tmp_path = path.with_file_name(format!(
            "{}.tmp",
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("credentials.json")
        ));
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(&data)?;
            file.sync_all()?;
        }
        if fs::rename(&tmp_path, path).is_err() {
            if path.exists() {
                let _ = fs::remove_file(path);
            }
            fs::rename(&tmp_path, path)?;
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            if let Ok(user) = std::env::var("USERNAME") {
                let trimmed = user.trim();
                if !trimmed.is_empty() {
                    let grant = format!("{trimmed}:(R,W)");
                    let _ = std::process::Command::new("icacls")
                        .arg(path)
                        .arg("/inheritance:r")
                        .arg("/grant:r")
                        .arg(&grant)
                        .creation_flags(CREATE_NO_WINDOW)
                        .output();
                }
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }

        Ok(())
    }

    fn save_field_at(path: &Path, field: CredentialField, value: &str) -> Result<(), StorageError> {
        let _guard = CREDENTIALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut creds = Self::read_file_unlocked(path);
        let cleaned = value.trim().to_string();
        let opt = if cleaned.is_empty() { None } else { Some(cleaned) };
        match field {
            CredentialField::Garmin => creds.garmin_personal_api_key = opt,
            CredentialField::Gemini => creds.gemini_api_key = opt,
            CredentialField::UIServer => creds.uiserver_auth_token = opt,
        }
        Self::write_file_unlocked(path, &creds)
    }

    fn read_field_at(path: &Path, field: CredentialField) -> Option<String> {
        let _guard = CREDENTIALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let creds = Self::read_file_unlocked(path);
        let val = match field {
            CredentialField::Garmin => creds.garmin_personal_api_key,
            CredentialField::Gemini => creds.gemini_api_key,
            CredentialField::UIServer => creds.uiserver_auth_token,
        };
        val.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    fn delete_field_at(path: &Path, field: CredentialField) -> Result<(), StorageError> {
        let _guard = CREDENTIALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if !path.exists() {
            return Ok(());
        }
        let mut creds = Self::read_file_unlocked(path);
        match field {
            CredentialField::Garmin => creds.garmin_personal_api_key = None,
            CredentialField::Gemini => creds.gemini_api_key = None,
            CredentialField::UIServer => creds.uiserver_auth_token = None,
        }
        Self::write_file_unlocked(path, &creds)
    }

    pub fn delete_credentials_file_at(path: &Path) {
        let _guard = CREDENTIALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        let tmp_path = path.with_file_name(format!(
            "{}.tmp",
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("credentials.json")
        ));
        if tmp_path.exists() {
            let _ = fs::remove_file(tmp_path);
        }
    }

    pub fn save_garmin_key_at(path: &Path, value: &str) -> Result<(), StorageError> {
        Self::save_field_at(path, CredentialField::Garmin, value)
    }

    pub fn read_garmin_key_at(path: &Path) -> Option<String> {
        Self::read_field_at(path, CredentialField::Garmin)
    }

    pub fn delete_garmin_key_at(path: &Path) -> Result<(), StorageError> {
        Self::delete_field_at(path, CredentialField::Garmin)
    }

    pub fn save_gemini_key_at(path: &Path, value: &str) -> Result<(), StorageError> {
        Self::save_field_at(path, CredentialField::Gemini, value)
    }

    pub fn read_gemini_key_at(path: &Path) -> Option<String> {
        Self::read_field_at(path, CredentialField::Gemini)
    }

    pub fn delete_gemini_key_at(path: &Path) -> Result<(), StorageError> {
        Self::delete_field_at(path, CredentialField::Gemini)
    }

    pub fn save_uiserver_token_at(path: &Path, value: &str) -> Result<(), StorageError> {
        Self::save_field_at(path, CredentialField::UIServer, value)
    }

    pub fn read_uiserver_token_at(path: &Path) -> Option<String> {
        Self::read_field_at(path, CredentialField::UIServer)
    }

    pub fn delete_uiserver_token_at(path: &Path) -> Result<(), StorageError> {
        Self::delete_field_at(path, CredentialField::UIServer)
    }

    fn read_legacy_keyring(service: &str, account: &str) -> Option<String> {
        let entry = keyring::Entry::new(service, account).ok()?;
        entry
            .get_password()
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn delete_legacy_keyring(service: &str, account: &str) {
        if let Ok(entry) = keyring::Entry::new(service, account) {
            let _ = entry.delete_credential();
        }
    }

    pub fn save_garmin_key(value: &str) -> Result<(), StorageError> {
        let path = Self::default_credentials_path();
        Self::save_garmin_key_at(&path, value)
    }

    pub fn read_garmin_key() -> Option<String> {
        let path = Self::default_credentials_path();
        if let Some(val) = Self::read_garmin_key_at(&path) {
            return Some(val);
        }
        // One-way legacy migration from Windows Credential Manager into credentials.json
        for svc in [GARMIN_KEYRING_SERVICE, APP_SERVICE] {
            if let Some(legacy) = Self::read_legacy_keyring(svc, GARMIN_ACCOUNT) {
                let _ = Self::save_garmin_key_at(&path, &legacy);
                Self::delete_legacy_keyring(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT);
                Self::delete_legacy_keyring(APP_SERVICE, GARMIN_ACCOUNT);
                return Some(legacy);
            }
        }
        None
    }

    pub fn delete_garmin_key() -> Result<(), StorageError> {
        let path = Self::default_credentials_path();
        Self::delete_garmin_key_at(&path)?;
        Self::delete_legacy_keyring(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT);
        Self::delete_legacy_keyring(APP_SERVICE, GARMIN_ACCOUNT);
        Ok(())
    }

    pub fn save_gemini_key(value: &str) -> Result<(), StorageError> {
        let path = Self::default_credentials_path();
        Self::save_gemini_key_at(&path, value)
    }

    pub fn read_gemini_key() -> Option<String> {
        let path = Self::default_credentials_path();
        if let Some(val) = Self::read_gemini_key_at(&path) {
            return Some(val);
        }
        if let Some(legacy) = Self::read_legacy_keyring(APP_SERVICE, GEMINI_ACCOUNT) {
            let _ = Self::save_gemini_key_at(&path, &legacy);
            Self::delete_legacy_keyring(APP_SERVICE, GEMINI_ACCOUNT);
            return Some(legacy);
        }
        None
    }

    pub fn delete_gemini_key() -> Result<(), StorageError> {
        let path = Self::default_credentials_path();
        Self::delete_gemini_key_at(&path)?;
        Self::delete_legacy_keyring(APP_SERVICE, GEMINI_ACCOUNT);
        Ok(())
    }

    pub fn save_uiserver_token(value: &str) -> Result<(), StorageError> {
        let path = Self::default_credentials_path();
        Self::save_uiserver_token_at(&path, value)
    }

    pub fn read_uiserver_token() -> Option<String> {
        let path = Self::default_credentials_path();
        if let Some(val) = Self::read_uiserver_token_at(&path) {
            return Some(val);
        }
        if let Some(legacy) = Self::read_legacy_keyring(APP_SERVICE, UISERVER_ACCOUNT) {
            let _ = Self::save_uiserver_token_at(&path, &legacy);
            Self::delete_legacy_keyring(APP_SERVICE, UISERVER_ACCOUNT);
            return Some(legacy);
        }
        None
    }

    pub fn delete_uiserver_token() -> Result<(), StorageError> {
        let path = Self::default_credentials_path();
        Self::delete_uiserver_token_at(&path)?;
        Self::delete_legacy_keyring(APP_SERVICE, UISERVER_ACCOUNT);
        Ok(())
    }

    pub fn save_secret(_service: &str, account: &str, value: &str) -> Result<(), StorageError> {
        match CredentialField::from_account(account) {
            CredentialField::Garmin => Self::save_garmin_key(value),
            CredentialField::Gemini => Self::save_gemini_key(value),
            CredentialField::UIServer => Self::save_uiserver_token(value),
        }
    }

    pub fn read_secret(_service: &str, account: &str) -> Option<String> {
        match CredentialField::from_account(account) {
            CredentialField::Garmin => Self::read_garmin_key(),
            CredentialField::Gemini => Self::read_gemini_key(),
            CredentialField::UIServer => Self::read_uiserver_token(),
        }
    }

    pub fn delete_secret(_service: &str, account: &str) -> Result<(), StorageError> {
        match CredentialField::from_account(account) {
            CredentialField::Garmin => Self::delete_garmin_key(),
            CredentialField::Gemini => Self::delete_gemini_key(),
            CredentialField::UIServer => Self::delete_uiserver_token(),
        }
    }

    pub fn clear_all() -> Result<(), StorageError> {
        Self::delete_credentials_file_at(&Self::default_credentials_path());
        Self::delete_legacy_keyring(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT);
        Self::delete_legacy_keyring(APP_SERVICE, GARMIN_ACCOUNT);
        Self::delete_legacy_keyring(APP_SERVICE, GEMINI_ACCOUNT);
        Self::delete_legacy_keyring(APP_SERVICE, UISERVER_ACCOUNT);
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
                body_battery_low INTEGER,
                body_battery_charged INTEGER,
                body_battery_drained INTEGER,
                body_battery_series_json TEXT,
                steps INTEGER,
                step_goal INTEGER,
                sleep_seconds INTEGER,
                sleep_duration_sec INTEGER,
                sleep_score INTEGER,
                sleep_start TEXT,
                sleep_end TEXT,
                sleep_deep_sec INTEGER,
                sleep_light_sec INTEGER,
                sleep_rem_sec INTEGER,
                sleep_awake_sec INTEGER,
                hrv_last_night INTEGER,
                hrv_status TEXT,
                hrv_weekly_avg INTEGER,
                hrv_baseline_low INTEGER,
                hrv_baseline_high INTEGER
            );
            CREATE TABLE IF NOT EXISTS briefings (
                date TEXT PRIMARY KEY,
                headline TEXT NOT NULL,
                body TEXT NOT NULL,
                action_cue TEXT NOT NULL,
                model_id TEXT NOT NULL,
                thinking_level TEXT NOT NULL,
                had_sleep INTEGER NOT NULL DEFAULT 0,
                generated_at TEXT NOT NULL
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

        for col_sql in [
            "ALTER TABLE days ADD COLUMN sleep_duration_sec INTEGER",
            "ALTER TABLE days ADD COLUMN sleep_start TEXT",
            "ALTER TABLE days ADD COLUMN sleep_end TEXT",
            "ALTER TABLE days ADD COLUMN sleep_deep_sec INTEGER",
            "ALTER TABLE days ADD COLUMN sleep_light_sec INTEGER",
            "ALTER TABLE days ADD COLUMN sleep_rem_sec INTEGER",
            "ALTER TABLE days ADD COLUMN sleep_awake_sec INTEGER",
            "ALTER TABLE days ADD COLUMN body_battery_low INTEGER",
            "ALTER TABLE days ADD COLUMN body_battery_charged INTEGER",
            "ALTER TABLE days ADD COLUMN body_battery_drained INTEGER",
            "ALTER TABLE days ADD COLUMN body_battery_series_json TEXT",
            "ALTER TABLE days ADD COLUMN hrv_weekly_avg INTEGER",
            "ALTER TABLE days ADD COLUMN hrv_baseline_low INTEGER",
            "ALTER TABLE days ADD COLUMN hrv_baseline_high INTEGER",
        ] {
            let _ = conn.execute(col_sql, []);
        }

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
        let optional_tables: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('briefings', 'chat_messages')",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.filter_map(Result::ok).collect()
        };
        for table in optional_tables {
            conn.execute(&format!("DELETE FROM {table}"), [])?;
        }
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
        let sleep_sec = day.sleep_seconds.or(day.sleep_duration_sec);
        let bb_series_json = day
            .body_battery_series
            .as_ref()
            .filter(|v| !v.is_empty())
            .and_then(|v| serde_json::to_string(v).ok());
        self.conn.lock().unwrap().execute(
            "INSERT INTO days (
                date, resting_hr, stress_avg, body_battery_high, body_battery_latest,
                body_battery_low, body_battery_charged, body_battery_drained, body_battery_series_json,
                steps, step_goal, sleep_seconds, sleep_duration_sec, sleep_score,
                sleep_start, sleep_end, sleep_deep_sec, sleep_light_sec, sleep_rem_sec, sleep_awake_sec,
                hrv_last_night, hrv_status, hrv_weekly_avg, hrv_baseline_low, hrv_baseline_high
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
            ON CONFLICT(date) DO UPDATE SET
                resting_hr=COALESCE(excluded.resting_hr, days.resting_hr),
                stress_avg=COALESCE(excluded.stress_avg, days.stress_avg),
                body_battery_high=COALESCE(excluded.body_battery_high, days.body_battery_high),
                body_battery_latest=COALESCE(excluded.body_battery_latest, days.body_battery_latest),
                body_battery_low=COALESCE(excluded.body_battery_low, days.body_battery_low),
                body_battery_charged=COALESCE(excluded.body_battery_charged, days.body_battery_charged),
                body_battery_drained=COALESCE(excluded.body_battery_drained, days.body_battery_drained),
                body_battery_series_json=COALESCE(excluded.body_battery_series_json, days.body_battery_series_json),
                steps=COALESCE(excluded.steps, days.steps),
                step_goal=COALESCE(excluded.step_goal, days.step_goal),
                sleep_seconds=COALESCE(excluded.sleep_seconds, days.sleep_seconds),
                sleep_duration_sec=COALESCE(excluded.sleep_duration_sec, days.sleep_duration_sec),
                sleep_score=COALESCE(excluded.sleep_score, days.sleep_score),
                sleep_start=COALESCE(excluded.sleep_start, days.sleep_start),
                sleep_end=COALESCE(excluded.sleep_end, days.sleep_end),
                sleep_deep_sec=COALESCE(excluded.sleep_deep_sec, days.sleep_deep_sec),
                sleep_light_sec=COALESCE(excluded.sleep_light_sec, days.sleep_light_sec),
                sleep_rem_sec=COALESCE(excluded.sleep_rem_sec, days.sleep_rem_sec),
                sleep_awake_sec=COALESCE(excluded.sleep_awake_sec, days.sleep_awake_sec),
                hrv_last_night=COALESCE(excluded.hrv_last_night, days.hrv_last_night),
                hrv_status=COALESCE(excluded.hrv_status, days.hrv_status),
                hrv_weekly_avg=COALESCE(excluded.hrv_weekly_avg, days.hrv_weekly_avg),
                hrv_baseline_low=COALESCE(excluded.hrv_baseline_low, days.hrv_baseline_low),
                hrv_baseline_high=COALESCE(excluded.hrv_baseline_high, days.hrv_baseline_high)",
            params![
                day.date,
                day.resting_hr,
                day.stress_avg,
                day.body_battery_high,
                day.body_battery_latest,
                day.body_battery_low,
                day.body_battery_charged,
                day.body_battery_drained,
                bb_series_json,
                day.steps,
                day.step_goal,
                sleep_sec,
                sleep_sec,
                day.sleep_score,
                day.sleep_start,
                day.sleep_end,
                day.sleep_deep_sec,
                day.sleep_light_sec,
                day.sleep_rem_sec,
                day.sleep_awake_sec,
                day.hrv_last_night,
                day.hrv_status,
                day.hrv_weekly_avg,
                day.hrv_baseline_low,
                day.hrv_baseline_high
            ],
        )?;
        Ok(())
    }

    pub fn list_days(&self, limit: usize) -> Result<Vec<DayRecord>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT date, resting_hr, stress_avg, body_battery_high, body_battery_latest,
                    body_battery_low, body_battery_charged, body_battery_drained, body_battery_series_json,
                    steps, step_goal, sleep_seconds, sleep_duration_sec, sleep_score,
                    sleep_start, sleep_end, sleep_deep_sec, sleep_light_sec, sleep_rem_sec, sleep_awake_sec,
                    hrv_last_night, hrv_status, hrv_weekly_avg, hrv_baseline_low, hrv_baseline_high
             FROM days ORDER BY date DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            let bb_series_str: Option<String> = r.get(8)?;
            let s1: Option<i32> = r.get(11)?;
            let s2: Option<i32> = r.get(12)?;
            let sleep_sec = s1.or(s2);
            Ok(DayRecord {
                date: r.get(0)?,
                resting_hr: r.get(1)?,
                stress_avg: r.get(2)?,
                body_battery_high: r.get(3)?,
                body_battery_latest: r.get(4)?,
                body_battery_low: r.get(5)?,
                body_battery_charged: r.get(6)?,
                body_battery_drained: r.get(7)?,
                body_battery_series: bb_series_str
                    .and_then(|s| serde_json::from_str::<Vec<BodyBatteryPoint>>(&s).ok())
                    .filter(|v| !v.is_empty()),
                steps: r.get(9)?,
                step_goal: r.get(10)?,
                sleep_seconds: sleep_sec,
                sleep_duration_sec: sleep_sec,
                sleep_score: r.get(13)?,
                sleep_start: r.get(14)?,
                sleep_end: r.get(15)?,
                sleep_deep_sec: r.get(16)?,
                sleep_light_sec: r.get(17)?,
                sleep_rem_sec: r.get(18)?,
                sleep_awake_sec: r.get(19)?,
                hrv_last_night: r.get(20)?,
                hrv_status: r.get(21)?,
                hrv_weekly_avg: r.get(22)?,
                hrv_baseline_low: r.get(23)?,
                hrv_baseline_high: r.get(24)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_daily_briefing(&self, date: &str) -> Result<Option<DailyBriefingRecord>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT date, headline, body, action_cue, model_id, thinking_level, had_sleep, generated_at
             FROM briefings WHERE date = ?1 LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![date], |r| {
            let had_sleep_int: i64 = r.get(6)?;
            Ok(DailyBriefingRecord {
                date: r.get(0)?,
                headline: r.get(1)?,
                body: r.get(2)?,
                action_cue: r.get(3)?,
                model_id: r.get(4)?,
                thinking_level: r.get(5)?,
                had_sleep: had_sleep_int != 0,
                generated_at: r.get(7)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn upsert_daily_briefing(&self, briefing: &DailyBriefingRecord) -> Result<(), StorageError> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO briefings (
                date, headline, body, action_cue, model_id, thinking_level, had_sleep, generated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(date) DO UPDATE SET
                headline = excluded.headline,
                body = excluded.body,
                action_cue = excluded.action_cue,
                model_id = excluded.model_id,
                thinking_level = excluded.thinking_level,
                had_sleep = excluded.had_sleep,
                generated_at = excluded.generated_at",
            params![
                briefing.date,
                briefing.headline,
                briefing.body,
                briefing.action_cue,
                briefing.model_id,
                briefing.thinking_level,
                if briefing.had_sleep { 1i64 } else { 0i64 },
                briefing.generated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_theme_mode(&self) -> Result<String, StorageError> {
        match self.get_setting("theme_mode")?.as_deref() {
            Some("dark") => Ok("dark".to_string()),
            Some("light") => Ok("light".to_string()),
            _ => Ok("system".to_string()),
        }
    }

    pub fn set_theme_mode(&self, mode: &str) -> Result<String, StorageError> {
        let normalized = match mode.trim().to_ascii_lowercase().as_str() {
            "dark" => "dark",
            "light" => "light",
            _ => "system",
        };
        self.set_setting("theme_mode", Some(normalized))?;
        Ok(normalized.to_string())
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
