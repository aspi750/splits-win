pub mod coach;
pub mod forensic_extractor;
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
    let has_garmin_key = CredentialStore::read_secret(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT).is_some()
        || CredentialStore::read_secret(APP_SERVICE, GARMIN_ACCOUNT).is_some();
    let has_gemini_key = CredentialStore::read_secret(APP_SERVICE, GEMINI_ACCOUNT).is_some();
    let config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let readiness: Option<ReadinessState> = store.read_json("readiness_state.json").map_err(|e| e.to_string())?;
    let activities = db.list_activities(50).map_err(|e| e.to_string())?;
    let days = db.list_days(60).map_err(|e| e.to_string())?;
    let settings = state.settings.lock().unwrap().clone();
    let profile_md = store.read_text("COACH_PROFILE.md").map_err(|e| e.to_string())?.unwrap_or_default();
    let plan_md = store.read_text("TRAINING_PLAN.md").map_err(|e| e.to_string())?.unwrap_or_default();
    let needs_initial_coach_setup = profile_md.trim().is_empty() || plan_md.trim().is_empty();
    let user_profile = db.get_user_profile().unwrap_or_default();

    Ok(serde_json::json!({
        "has_garmin_key": has_garmin_key,
        "has_gemini_key": has_gemini_key,
        "engine_mode": coach::resolve_coach_engine_mode(),
        "shared_dir": store.root().display().to_string(),
        "settings": settings,
        "athlete_config": config,
        "user_profile": user_profile,
        "readiness_state": readiness,
        "activities": activities,
        "days": days,
        "needs_initial_coach_setup": needs_initial_coach_setup,
    }))
}

#[tauri::command]
fn save_garmin_key(key: String) -> Result<(), String> {
    CredentialStore::save_secret(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT, &key)
        .map_err(|e| e.to_string())?;
    let _ = CredentialStore::save_secret(APP_SERVICE, GARMIN_ACCOUNT, &key);
    Ok(())
}

#[tauri::command]
async fn verify_garmin_key(key: String) -> Result<serde_json::Value, String> {
    let trimmed = key.trim().to_string();
    if trimmed.is_empty() {
        return Err("Garmin Personal API key cannot be empty.".to_string());
    }
    let client = garmin::GarminClient::new(Some(trimmed.clone()));
    client.authenticate(true).await.map_err(|e| e.to_string())?;
    CredentialStore::save_secret(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT, &trimmed)
        .map_err(|e| e.to_string())?;
    let _ = CredentialStore::save_secret(APP_SERVICE, GARMIN_ACCOUNT, &trimmed);
    Ok(serde_json::json!({ "ok": true, "token_cached": true }))
}

#[tauri::command]
fn probe_fit_baseline(folder_path: String) -> Result<FitBaselineProbe, String> {
    garmin::probe_fit_folder(std::path::Path::new(&folder_path))
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
    let db = state.open_db()?;
    let initial_backfill = db.list_activities(1).map_err(|e| e.to_string())?.is_empty();
    let client = garmin::GarminClient::new(None);
    let (synced_activities, synced_days) = client
        .sync_lifetime_and_wellness(initial_backfill)
        .await
        .map_err(|e| e.to_string())?;

    let store = state.load_store()?;
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

fn attach_latest_forensic(
    packet: String,
    db: &Database,
    activities: &[ActivityRecord],
) -> String {
    let tracked = activities.iter().take(5).find_map(|act| {
        db.get_fit_track(act.garmin_id)
            .ok()
            .flatten()
            .map(|track| (act, track))
    });
    if let Some((act, track)) = tracked {
        coach::append_forensic_section(packet, Some(act), track.forensic_telemetry.as_ref())
    } else if let Some(act) = activities.first() {
        coach::append_forensic_section(packet, Some(act), None)
    } else {
        packet
    }
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

    let packet = coach::CoachPacketBuilder::build_markdown(
        &config,
        &activities,
        &days,
        gear.as_ref(),
        readiness.as_ref(),
        &draft_prompt,
        Utc::now(),
    );
    Ok(attach_latest_forensic(packet, &db, &activities))
}

#[tauri::command]
fn apply_coach_proposal(state: State<'_, AppState>, proposal: CoachUpdateProposal) -> Result<(), String> {
    let store = state.load_store()?;
    coach::apply_coach_proposal(&store, &proposal)
}

#[tauri::command]
fn save_user_profile(state: State<'_, AppState>, profile: UserProfile) -> Result<(), String> {
    let db = state.open_db()?;
    db.save_user_profile(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
fn reclassify_activity_as_hyrox(
    state: State<'_, AppState>,
    activity_id: i64,
    is_hyrox: bool,
) -> Result<ActivityDetailPayload, String> {
    let db = state.open_db()?;
    db.set_activity_hyrox(activity_id, is_hyrox).map_err(|e| e.to_string())?;
    let activity = db
        .get_activity(activity_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Activity {activity_id} not found"))?;
    let mut track = db.get_fit_track(activity_id).map_err(|e| e.to_string())?;
    if let Some(ref mut t) = track {
        t.hyrox_dissection = if is_hyrox {
            garmin::dissect_hyrox_laps(&t.laps, true)
        } else {
            None
        };
        let _ = db.upsert_fit_track(activity_id, t);
    }
    Ok(ActivityDetailPayload { activity, track })
}

#[tauri::command]
fn generate_starter_coach_files(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let store = state.load_store()?;
    let db = state.open_db()?;
    let config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let activities = db.list_activities(5000).map_err(|e| e.to_string())?;
    let summary = coach::analyze_training_history(&activities, &config, Utc::now());

    let profile_existing = store.read_text("COACH_PROFILE.md").map_err(|e| e.to_string())?.unwrap_or_default();
    let plan_existing = store.read_text("TRAINING_PLAN.md").map_err(|e| e.to_string())?.unwrap_or_default();

    if profile_existing.trim().is_empty() {
        let md = coach::starter_coach_profile_markdown(&config, &summary);
        store.write_text_atomic("COACH_PROFILE.md", &md).map_err(|e| e.to_string())?;
    }
    if plan_existing.trim().is_empty() {
        let md = coach::starter_training_plan_markdown(&config, &summary);
        store.write_text_atomic("TRAINING_PLAN.md", &md).map_err(|e| e.to_string())?;
    }
    Ok(serde_json::json!({ "ok": true, "days_of_history": summary.days_of_history }))
}

async fn load_or_fetch_fit_track(
    store: &SharedStore,
    db: &Database,
    activity_id: i64,
) -> Result<Option<FitTrackAnalysis>, String> {
    if let Some(mut track) = db.get_fit_track(activity_id).map_err(|e| e.to_string())? {
        if track.forensic_telemetry.is_none() && !track.full_points.is_empty() {
            track.forensic_telemetry = Some(forensic_extractor::extract_forensic_telemetry(
                &track.full_points,
                &[],
            ));
            let _ = db.upsert_fit_track(activity_id, &track);
        }
        return Ok(Some(track));
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
    Ok(track)
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
    let track = load_or_fetch_fit_track(&store, &db, activity_id).await?;
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
    let activities = db.list_activities(5000).map_err(|e| e.to_string())?;
    let days = db.list_days(365).map_err(|e| e.to_string())?;

    if let Some(api_key) = CredentialStore::read_secret(APP_SERVICE, GEMINI_ACCOUNT) {
        let user_profile = db.get_user_profile().ok();
        let sys = coach::build_static_system_instruction(
            &store,
            &config,
            &activities,
            user_profile.as_ref(),
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
        let packet = attach_latest_forensic(packet, &db, &activities);
        Ok(serde_json::json!({
            "mode": "clipboard_packet",
            "packet": packet,
            "proposals": [],
        }))
    }
}

fn opt_f64(value: Option<f64>, precision: usize) -> String {
    match value {
        Some(v) => format!("{v:.precision$}"),
        None => String::new(),
    }
}

#[tauri::command]
fn save_activity_post_run_context(
    state: State<'_, AppState>,
    activity_id: i64,
    context: PostRunContext,
) -> Result<ActivityRecord, String> {
    let db = state.open_db()?;
    let activity = db
        .get_activity(activity_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Activity {activity_id} not found"))?;
    let dur = if activity.moving_duration_sec > 0.0 {
        activity.moving_duration_sec
    } else {
        activity.duration_sec
    };
    let computed = context.with_computed_sweat(dur);
    db.save_post_run_context(activity_id, &computed)
        .map_err(|e| e.to_string())?;
    db.get_activity(activity_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Activity {activity_id} disappeared"))
}

#[tauri::command]
async fn export_1hz_csv(
    state: State<'_, AppState>,
    activity_id: i64,
    target_path: String,
) -> Result<usize, String> {
    let store = state.load_store()?;
    let db = state.open_db()?;
    let track = load_or_fetch_fit_track(&store, &db, activity_id)
        .await?
        .ok_or_else(|| format!("No 1Hz FIT track available for activity {activity_id}."))?;
    if track.full_points.is_empty() {
        return Err("Track contains zero 1Hz points.".to_string());
    }
    let mut lines = Vec::with_capacity(track.full_points.len() + 1);
    lines.push(
        "second,distance_m,pace_sec_per_km,speed_mps,hr_bpm,power_w,cadence_spm,gct_ms,stance_pct,vert_osc_mm,vert_ratio_pct,step_length_mm,stance_balance_left_pct,altitude_m,temp_c,respiration_brpm,lat,lon,impact_load"
            .to_string(),
    );
    for p in &track.full_points {
        lines.push(format!(
            "{:.1},{:.2},{},{:.3},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            p.timestamp_sec,
            p.distance_m,
            opt_f64(p.pace_sec_per_km, 1),
            p.speed_mps,
            opt_f64(p.heart_rate_bpm, 1),
            opt_f64(p.power_w, 1),
            opt_f64(p.cadence_spm, 1),
            opt_f64(p.gct_ms, 1),
            opt_f64(p.stance_time_pct, 2),
            opt_f64(p.vertical_oscillation_mm, 1),
            opt_f64(p.vertical_ratio_pct, 2),
            opt_f64(p.step_length_mm, 1),
            opt_f64(p.stance_balance_left_pct, 2),
            opt_f64(p.altitude_m, 1),
            opt_f64(p.temperature_c, 1),
            opt_f64(p.respiration_brpm, 1),
            opt_f64(p.latitude_deg, 6),
            opt_f64(p.longitude_deg, 6),
            opt_f64(p.impact_load, 2),
        ));
    }
    std::fs::write(&target_path, format!("{}\n", lines.join("\n"))).map_err(|e| e.to_string())?;
    Ok(track.full_points.len())
}

#[tauri::command]
fn import_training_ledger(state: State<'_, AppState>, source_path: String) -> Result<usize, String> {
    let store = state.load_store()?;
    let raw = std::fs::read_to_string(&source_path).map_err(|e| e.to_string())?;
    if raw.trim().is_empty() {
        return Err("Selected ledger file is empty.".to_string());
    }
    store
        .write_text_atomic("ASPI_Training_Log.txt", &raw)
        .map_err(|e| e.to_string())?;
    Ok(raw.lines().count())
}

#[tauri::command]
fn export_training_ledger(state: State<'_, AppState>, target_path: String) -> Result<usize, String> {
    let store = state.load_store()?;
    let raw = store
        .read_text("ASPI_Training_Log.txt")
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if raw.trim().is_empty() {
        return Err("Training ledger (ASPI_Training_Log.txt) is currently empty.".to_string());
    }
    std::fs::write(&target_path, &raw).map_err(|e| e.to_string())?;
    Ok(raw.lines().count())
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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
            verify_garmin_key,
            probe_fit_baseline,
            save_gemini_key,
            save_athlete_config,
            save_user_profile,
            reclassify_activity_as_hyrox,
            set_execution_mode,
            set_custom_shared_dir,
            recompute_readiness,
            sync_garmin_now,
            build_coach_packet,
            send_coach_message,
            apply_coach_proposal,
            generate_starter_coach_files,
            get_activity_detail,
            save_activity_post_run_context,
            export_1hz_csv,
            import_training_ledger,
            export_training_ledger,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Splits Coach");
}
