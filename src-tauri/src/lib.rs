pub mod coach;
pub mod garmin;
pub mod models;
pub mod readiness;
pub mod scheduler;
pub mod storage;

use chrono::Utc;
use models::*;
use readiness::{ReadinessEngine, ReadinessInput};
use std::path::PathBuf;
use std::sync::Mutex;
use storage::{AppSettings, CredentialStore, Database, SharedStore};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, State,
};

pub struct AppState {
    pub settings: Mutex<AppSettings>,
}

impl AppState {
    fn load_store(&self) -> Result<SharedStore, String> {
        let custom = self
            .settings
            .lock()
            .unwrap()
            .custom_shared_coach_directory_path
            .as_ref()
            .map(PathBuf::from);
        SharedStore::new(custom).map_err(|e| e.to_string())
    }

    fn open_db(&self) -> Result<Database, String> {
        let store = self.load_store()?;
        Database::open(&store.root().join("state").join("splits.sqlite")).map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn get_bootstrap_state(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let store = state.load_store()?;
    let db = state.open_db()?;
    let has_garmin_key = CredentialStore::read_secret(APP_SERVICE, GARMIN_ACCOUNT).is_some();
    let has_gemini_key = CredentialStore::read_secret(APP_SERVICE, GEMINI_ACCOUNT).is_some();
    let config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let readiness: Option<ReadinessState> = store.read_json("readiness_state.json").map_err(|e| e.to_string())?;
    let activities = db.list_activities(50).map_err(|e| e.to_string())?;
    let days = db.list_days(60).map_err(|e| e.to_string())?;
    let settings = state.settings.lock().unwrap().clone();

    Ok(serde_json::json!({
        "has_garmin_key": has_garmin_key,
        "has_gemini_key": has_gemini_key,
        "engine_mode": coach::resolve_coach_engine_mode(),
        "shared_dir": store.root().display().to_string(),
        "settings": settings,
        "athlete_config": config,
        "readiness_state": readiness,
        "activities": activities,
        "days": days,
    }))
}

#[tauri::command]
fn save_garmin_key(key: String) -> Result<(), String> {
    CredentialStore::save_secret(APP_SERVICE, GARMIN_ACCOUNT, &key).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_gemini_key(key: String) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        CredentialStore::delete_secret(APP_SERVICE, GEMINI_ACCOUNT).map_err(|e| e.to_string())
    } else {
        CredentialStore::save_secret(APP_SERVICE, GEMINI_ACCOUNT, trimmed).map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn save_athlete_config(state: State<'_, AppState>, config: AthleteConfig) -> Result<(), String> {
    let store = state.load_store()?;
    store.save_athlete_config(&config).map_err(|e| e.to_string())?;
    let mut s = state.settings.lock().unwrap();
    s.profile_onboarded = true;
    Ok(())
}

#[tauri::command]
fn set_execution_mode(state: State<'_, AppState>, mode: ExecutionMode) -> Result<(), String> {
    state.settings.lock().unwrap().execution_mode = mode;
    Ok(())
}

#[tauri::command]
fn set_custom_shared_dir(state: State<'_, AppState>, path: Option<String>) -> Result<String, String> {
    {
        let mut s = state.settings.lock().unwrap();
        s.custom_shared_coach_directory_path = path.filter(|p| !p.trim().is_empty());
    }
    let store = state.load_store()?;
    Ok(store.root().display().to_string())
}

#[tauri::command]
fn recompute_readiness(state: State<'_, AppState>) -> Result<ReadinessState, String> {
    let store = state.load_store()?;
    let db = state.open_db()?;
    let config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let activities = db.list_activities(200).map_err(|e| e.to_string())?;
    let days = db.list_days(60).map_err(|e| e.to_string())?;
    let plan: Option<PlanFile> = store.read_json("plan.json").map_err(|e| e.to_string())?;
    let gear: Option<GearInventoryFile> = store.read_json("gear_inventory.json").map_err(|e| e.to_string())?;

    let output = ReadinessEngine::evaluate(ReadinessInput {
        now: Utc::now(),
        athlete_config: &config,
        activities: &activities,
        days: &days,
        plan: plan.as_ref(),
        gear_inventory: gear.as_ref(),
    });
    output.persist(store.root()).map_err(|e| e.to_string())?;
    Ok(output.readiness_state)
}

#[tauri::command]
async fn sync_garmin_now(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let client = garmin::GarminClient::new(None);
    let (synced_activities, synced_days) = client.sync_recent(35).await.map_err(|e| e.to_string())?;

    let store = state.load_store()?;
    let db = state.open_db()?;
    let acts = synced_activities.len();
    let days_count = synced_days.len();
    for a in &synced_activities {
        let _ = db.upsert_activity(a);
    }
    for d in &synced_days {
        let _ = db.upsert_day(d);
    }

    let config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let activities = db.list_activities(200).map_err(|e| e.to_string())?;
    let days = db.list_days(60).map_err(|e| e.to_string())?;
    let output = ReadinessEngine::evaluate(ReadinessInput {
        now: Utc::now(),
        athlete_config: &config,
        activities: &activities,
        days: &days,
        plan: None,
        gear_inventory: None,
    });
    output.persist(store.root()).map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "synced_activities": acts,
        "synced_days": days_count,
        "readiness_state": output.readiness_state,
    }))
}

#[tauri::command]
fn build_coach_packet(state: State<'_, AppState>, draft_prompt: String) -> Result<String, String> {
    let store = state.load_store()?;
    let db = state.open_db()?;
    let config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let activities = db.list_activities(50).map_err(|e| e.to_string())?;
    let days = db.list_days(60).map_err(|e| e.to_string())?;
    let gear: Option<GearInventoryFile> = store.read_json("gear_inventory.json").map_err(|e| e.to_string())?;
    let readiness: Option<ReadinessState> = store.read_json("readiness_state.json").map_err(|e| e.to_string())?;

    Ok(coach::CoachPacketBuilder::build_markdown(
        &config,
        &activities,
        &days,
        gear.as_ref(),
        readiness.as_ref(),
        &draft_prompt,
        Utc::now(),
    ))
}

#[tauri::command]
fn apply_coach_proposal(state: State<'_, AppState>, proposal: CoachUpdateProposal) -> Result<(), String> {
    let store = state.load_store()?;
    coach::apply_coach_proposal(&store, &proposal)
}

#[tauri::command]
async fn get_activity_detail(
    state: State<'_, AppState>,
    activity_id: i64,
) -> Result<ActivityDetailPayload, String> {
    let store = state.load_store()?;
    let db = state.open_db()?;
    let activity = db
        .get_activity(activity_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Activity {activity_id} not found"))?;

    if let Some(track) = db.get_fit_track(activity_id).map_err(|e| e.to_string())? {
        return Ok(ActivityDetailPayload {
            activity,
            track: Some(track),
        });
    }

    // Lazily fetch & decode .fit if Garmin key is available
    let config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let client = garmin::GarminClient::new(None);
    let track = match client.download_fit_bytes(activity_id).await {
        Ok(bytes) => match garmin::parse_fit_track(activity_id, &bytes, &config) {
            Ok(parsed) => {
                let _ = db.upsert_fit_track(activity_id, &parsed);
                Some(parsed)
            }
            Err(_) => None,
        },
        Err(_) => None,
    };

    Ok(ActivityDetailPayload { activity, track })
}

#[tauri::command]
async fn send_coach_message(
    app: AppHandle,
    state: State<'_, AppState>,
    prompt: String,
) -> Result<serde_json::Value, String> {
    let store = state.load_store()?;
    let db = state.open_db()?;
    let config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let activities = db.list_activities(50).map_err(|e| e.to_string())?;
    let days = db.list_days(60).map_err(|e| e.to_string())?;

    if let Some(api_key) = CredentialStore::read_secret(APP_SERVICE, GEMINI_ACCOUNT) {
        let sys = format!(
            "You are a training-planning coach inside Splits Coach. Use metric units.\n\n{}",
            config.prompt_summary(None)
        );
        let (text, proposals) =
            coach::run_gemini_coach_turn(Some(&app), &api_key, &sys, &prompt, &store, Some(&db), &activities, &days)
                .await?;
        Ok(serde_json::json!({
            "mode": "gemini_api",
            "text": text,
            "proposals": proposals,
        }))
    } else {
        let gear: Option<GearInventoryFile> = store.read_json("gear_inventory.json").map_err(|e| e.to_string())?;
        let readiness: Option<ReadinessState> = store.read_json("readiness_state.json").map_err(|e| e.to_string())?;
        let packet = coach::CoachPacketBuilder::build_markdown(
            &config,
            &activities,
            &days,
            gear.as_ref(),
            readiness.as_ref(),
            &prompt,
            Utc::now(),
        );
        Ok(serde_json::json!({
            "mode": "clipboard_packet",
            "packet": packet,
            "proposals": [],
        }))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(AppState {
            settings: Mutex::new(AppSettings::default()),
        })
        .setup(|app| {
            let show_i = MenuItem::with_id(app, "show", "Open Splits Coach", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;
            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_bootstrap_state,
            save_garmin_key,
            save_gemini_key,
            save_athlete_config,
            set_execution_mode,
            set_custom_shared_dir,
            recompute_readiness,
            sync_garmin_now,
            build_coach_packet,
            send_coach_message,
            apply_coach_proposal,
            get_activity_detail,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Splits Coach");
}
