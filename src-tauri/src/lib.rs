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
    let model_settings = db.load_model_settings().unwrap_or_default();
    let first_use_state = store.load_first_use_state().unwrap_or_default();

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
        "model_settings": model_settings,
        "first_use_state": first_use_state,
    }))
}

#[tauri::command]
fn save_garmin_key(key: String) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        CredentialStore::delete_secret(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT).map_err(|e| e.to_string())?;
        let _ = CredentialStore::delete_secret(APP_SERVICE, GARMIN_ACCOUNT);
        Ok(())
    } else {
        CredentialStore::save_secret(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT, trimmed)
            .map_err(|e| e.to_string())?;
        let _ = CredentialStore::save_secret(APP_SERVICE, GARMIN_ACCOUNT, trimmed);
        Ok(())
    }
}

#[tauri::command]
async fn verify_garmin_key(key: String) -> Result<serde_json::Value, String> {
    let trimmed = key.trim().to_string();
    if trimmed.is_empty() {
        return Err("Garmin Personal API key cannot be empty.".to_string());
    }
    let client = garmin::GarminClient::new(Some(trimmed.clone()));
    if let Err(err) = client.authenticate(true).await {
        return match err {
            garmin::GarminError::BadKey => {
                Err("HTTP 401/403 Invalid API Key — check your Garmin Personal API key.".to_string())
            }
            other => Err(other.to_string()),
        };
    }
    CredentialStore::save_secret(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT, &trimmed)
        .map_err(|e| e.to_string())?;
    let _ = CredentialStore::save_secret(APP_SERVICE, GARMIN_ACCOUNT, &trimmed);
    let read_back = CredentialStore::read_secret(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT)
        .or_else(|| CredentialStore::read_secret(APP_SERVICE, GARMIN_ACCOUNT));
    if read_back.as_deref() != Some(trimmed.as_str()) {
        return Err("Garmin key saved but could not be verified on read-back from Credential Manager.".to_string());
    }
    Ok(serde_json::json!({ "ok": true, "http_status": 200, "token_cached": true, "persisted": true }))
}

#[tauri::command]
async fn verify_gemini_key(
    key: Option<String>,
    persist: Option<bool>,
) -> Result<serde_json::Value, String> {
    let candidate = match key.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None => CredentialStore::read_secret(APP_SERVICE, GEMINI_ACCOUNT)
            .ok_or_else(|| "Enter or save a Gemini API key first.".to_string())?,
    };
    let message = coach::verify_gemini_api_key(&candidate).await?;
    let should_persist = persist.unwrap_or(true);
    let mut persisted = false;
    if should_persist && key.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some() {
        CredentialStore::save_secret(APP_SERVICE, GEMINI_ACCOUNT, &candidate)
            .map_err(|e| e.to_string())?;
        let read_back = CredentialStore::read_secret(APP_SERVICE, GEMINI_ACCOUNT);
        if read_back.as_deref() != Some(candidate.as_str()) {
            return Err("Gemini API key could not be verified on read-back from Credential Manager.".to_string());
        }
        persisted = true;
    } else if CredentialStore::read_secret(APP_SERVICE, GEMINI_ACCOUNT).as_deref() == Some(candidate.as_str()) {
        persisted = true;
    }
    Ok(serde_json::json!({
        "ok": true,
        "http_status": 200,
        "message": message,
        "persisted": persisted,
    }))
}

#[tauri::command]
fn clean_slate_reset(state: State<'_, AppState>) -> Result<(), String> {
    let store = state.load_store()?;
    {
        let db = state.open_db()?;
        db.clean_slate_reset().map_err(|e| e.to_string())?;
    }
    let state_dir = store.root().join("state");
    for db_file in ["splits.sqlite", "splits.sqlite-wal", "splits.sqlite-shm"] {
        let p = state_dir.join(db_file);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    store.clean_slate_files().map_err(|e| e.to_string())?;
    let base_dir = SharedStore::default_base_dir();
    let cred_file = base_dir.join("credentials.json");
    if cred_file.exists() {
        let _ = std::fs::remove_file(&cred_file);
    }
    for cache_dir in ["cache", "Cache", "CoachCache", "Activities", "logs"] {
        let p = base_dir.join(cache_dir);
        if p.exists() {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
    if let Ok(entries) = std::fs::read_dir(&base_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map_or(false, |e| e.eq_ignore_ascii_case("fit"))
            {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    let _ = CredentialStore::clear_all();
    *state.settings.lock().unwrap() = AppSettings::default();
    let _ = state.open_db()?;
    Ok(())
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

async fn perform_full_garmin_sync(
    store: &SharedStore,
    db: &Database,
) -> Result<(usize, usize, usize, ReadinessState), String> {
    let initial_backfill = db.list_activities(1).map_err(|e| e.to_string())?.is_empty();
    let client = garmin::GarminClient::new(None);
    let (synced_activities, synced_days) = client
        .sync_lifetime_and_wellness(initial_backfill)
        .await
        .map_err(|e| e.to_string())?;

    let synced_weights = client
        .sync_weights(if initial_backfill { 365 } else { 35 })
        .await;

    let acts = synced_activities.len();
    let days_count = synced_days.len();
    let weights_count = synced_weights.len();
    for a in &synced_activities {
        let _ = db.upsert_activity(a);
    }
    for d in &synced_days {
        let _ = db.upsert_day(d);
    }
    for w in &synced_weights {
        let _ = db.upsert_weight_entry(w);
    }

    // Lightweight metadata-only upsert on rolling last 7 days to capture late Garmin Connect edits/notes
    if let Ok(recent_meta) = client.fetch_recent_activity_metadata(7).await {
        for m in recent_meta {
            let _ = db.update_activity_metadata(
                m.garmin_id,
                m.name.as_deref(),
                m.garmin_notes.as_deref(),
            );
        }
    }

    let mut config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let all_weights = db.list_weight_entries(400).unwrap_or_default();
    let mut activities = db.list_activities(200).map_err(|e| e.to_string())?;
    for a in &mut activities {
        if let Some(start_ep) = scale_matcher::parse_activity_epoch_sec(&a.start_iso) {
            let dur = a.moving_duration_sec.max(a.duration_sec).max(0.0);
            let end_ep = start_ep + a.duration_sec.max(a.moving_duration_sec).max(0.0).round() as i64;
            let current = a.post_run_context.clone().unwrap_or_default();
            let paired = scale_matcher::pair_scale_weigh_ins(
                start_ep,
                end_ep,
                dur,
                &all_weights,
                current.clone(),
            );
            if paired != current {
                let _ = db.save_post_run_context(a.garmin_id, &paired);
                a.post_run_context = Some(paired);
            }
        }
    }

    let windows = scale_matcher::activity_windows(&activities);
    if let Some(baseline_kg) = scale_matcher::latest_baseline_weight_kg(&all_weights, &windows) {
        if (!config.specs.weight_baseline_confirmed || config.specs.weight_kg.unwrap_or(0.0) <= 0.0)
            && (config.specs.weight_kg.unwrap_or(0.0) - baseline_kg).abs() >= 0.05
        {
            config.specs.weight_kg = Some(baseline_kg);
            let _ = store.save_athlete_config(&config);
        }
    }

    let days = db.list_days(60).map_err(|e| e.to_string())?;
    let plan: Option<PlanFile> = store.read_json("plan.json").ok().flatten();
    let gear: Option<GearInventoryFile> = store.read_json("gear_inventory.json").ok().flatten();
    let output = ReadinessEngine::evaluate(ReadinessInput {
        now: Utc::now(),
        athlete_config: &config,
        activities: &activities,
        days: &days,
        plan: plan.as_ref(),
        gear_inventory: gear.as_ref(),
    });
    output.persist(store.root()).map_err(|e| e.to_string())?;

    Ok((acts, days_count, weights_count, output.readiness_state))
}

#[tauri::command]
async fn sync_garmin_now(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let store = state.load_store()?;
    let db = state.open_db()?;
    let (acts, days_count, weights_count, readiness_state) =
        perform_full_garmin_sync(&store, &db).await?;

    Ok(serde_json::json!({
        "synced_activities": acts,
        "synced_days": days_count,
        "synced_weights": weights_count,
        "readiness_state": readiness_state,
    }))
}

fn attach_latest_forensic(
    packet: String,
    db: &Database,
    config: &AthleteConfig,
    activities: &[ActivityRecord],
) -> String {
    let weights = db.list_weight_entries(60).unwrap_or_default();
    let windows = scale_matcher::activity_windows(activities);
    let tracked = activities.iter().take(5).find_map(|act| {
        db.get_fit_track(act.garmin_id)
            .ok()
            .flatten()
            .map(|track| (act, track))
    });
    let target_act = tracked.as_ref().map(|(a, _)| *a).or_else(|| activities.first());
    let ref_epoch = target_act
        .and_then(|a| scale_matcher::parse_activity_epoch_sec(&a.start_iso))
        .unwrap_or_else(|| Utc::now().timestamp());
    let resolved_weight = scale_matcher::resolve_effective_weight(
        target_act.and_then(|a| a.post_run_context.as_ref()),
        ref_epoch,
        &weights,
        &windows,
        config.specs.weight_kg,
    );
    if let Some((act, track)) = tracked {
        coach::append_forensic_section(
            packet,
            Some(act),
            track.forensic_telemetry.as_ref(),
            resolved_weight.as_ref(),
        )
    } else if let Some(act) = activities.first() {
        coach::append_forensic_section(packet, Some(act), None, resolved_weight.as_ref())
    } else {
        coach::append_forensic_section(packet, None, None, resolved_weight.as_ref())
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
    Ok(attach_latest_forensic(packet, &db, &config, &activities))
}

#[tauri::command]
fn apply_coach_proposal(state: State<'_, AppState>, proposal: CoachUpdateProposal) -> Result<(), String> {
    let store = state.load_store()?;
    let db = state.open_db()?;
    coach::apply_coach_proposal(&store, &proposal)?;

    let mut first_use = store.load_first_use_state().map_err(|e| e.to_string())?;
    if proposal.target_file == "COACH_PROFILE.md" {
        let mut user_profile = db.get_user_profile().unwrap_or_default();
        let mut config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
        if coach::hydrate_profile_from_proposal(&proposal, &mut user_profile, &mut config) {
            db.save_user_profile(&user_profile).map_err(|e| e.to_string())?;
            store.save_athlete_config(&config).map_err(|e| e.to_string())?;
        }
        if first_use.stage == FirstUseStage::ProfileApproval {
            first_use.stage = FirstUseStage::HistorySync;
            store.save_first_use_state(&first_use).map_err(|e| e.to_string())?;
        }
    } else if proposal.target_file == "TRAINING_PLAN.md" {
        let config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
        let activities = db.list_activities(5000).map_err(|e| e.to_string())?;
        let days = db.list_days(60).map_err(|e| e.to_string())?;
        let mut plan: Option<PlanFile> = store.read_json("plan.json").ok().flatten();
        if plan.is_none() {
            let summary = coach::analyze_training_history(&activities, &config, Utc::now());
            let seeded = coach::starter_plan_file(&config, &summary);
            let _ = store.write_json_atomic("plan.json", &seeded);
            plan = Some(seeded);
        }
        let gear: Option<GearInventoryFile> = store.read_json("gear_inventory.json").ok().flatten();
        let output = ReadinessEngine::evaluate(ReadinessInput {
            now: Utc::now(),
            athlete_config: &config,
            activities: &activities,
            days: &days,
            plan: plan.as_ref(),
            gear_inventory: gear.as_ref(),
        });
        let _ = output.persist(store.root());
        if first_use.stage == FirstUseStage::PlanApproval {
            first_use.stage = FirstUseStage::Active;
            store.save_first_use_state(&first_use).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
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
            track.forensic_telemetry = Some(forensic_extractor::extract_forensic_telemetry_with_source(
                &track.full_points,
                &[],
                &track.temp_source,
            ));
            let _ = db.upsert_fit_track(activity_id, &track);
        }
        let _ = db.set_activity_fit_temp_source(activity_id, &track.temp_source);
        return Ok(Some(track));
    }

    // Lazily fetch & decode .fit if Garmin key is available
    let config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let client = garmin::GarminClient::new(None);
    let track = match client.download_fit_bytes(activity_id).await {
        Ok(bytes) => match garmin::parse_fit_track(activity_id, &bytes, &config) {
            Ok(parsed) => {
                let _ = db.upsert_fit_track(activity_id, &parsed);
                let _ = db.set_activity_fit_temp_source(activity_id, &parsed.temp_source);
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
    let track = load_or_fetch_fit_track(&store, &db, activity_id).await?;
    let activity = db
        .get_activity(activity_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Activity {activity_id} not found"))?;
    Ok(ActivityDetailPayload { activity, track })
}

#[tauri::command]
fn save_global_model_settings(
    state: State<'_, AppState>,
    model_id: String,
    thinking_level: String,
) -> Result<ModelSettings, String> {
    let db = state.open_db()?;
    let level = ThinkingLevel::from_str_lossy(&thinking_level).unwrap_or(ThinkingLevel::Medium);
    db.save_global_model_settings(&model_id, level)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn save_coach_model_override(
    state: State<'_, AppState>,
    model_id: Option<String>,
    thinking_level: Option<String>,
) -> Result<ModelSettings, String> {
    let db = state.open_db()?;
    let parsed_thinking = thinking_level
        .as_deref()
        .and_then(ThinkingLevel::from_str_lossy);
    db.save_coach_model_override(model_id.as_deref(), parsed_thinking)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn refresh_activity_metadata(
    state: State<'_, AppState>,
    activity_id: i64,
) -> Result<ActivityRecord, String> {
    let db = state.open_db()?;
    let client = garmin::GarminClient::new(None);
    let update = client
        .fetch_activity_metadata(activity_id)
        .await
        .map_err(|e| e.to_string())?;
    let _ = db
        .update_activity_metadata(
            update.garmin_id,
            update.name.as_deref(),
            update.garmin_notes.as_deref(),
        )
        .map_err(|e| e.to_string())?;
    db.get_activity(activity_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Activity {activity_id} not found"))
}

#[tauri::command]
fn queue_intake_file(
    state: State<'_, AppState>,
    source_path: String,
) -> Result<FirstUseFlowState, String> {
    let store = state.load_store()?;
    let src = std::path::Path::new(&source_path);
    if !src.exists() {
        return Err(format!("Selected file does not exist: {source_path}"));
    }
    let orig_name = src
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("intake_file")
        .to_string();
    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let (mime_type, kind) = match ext.as_str() {
        "pdf" => ("application/pdf".to_string(), "lab_stress_pdf".to_string()),
        "fit" => ("application/octet-stream".to_string(), "garmin_fit".to_string()),
        "md" => ("text/markdown".to_string(), "medical_notes".to_string()),
        "txt" => ("text/plain".to_string(), "medical_notes".to_string()),
        _ => {
            return Err(format!(
                "Unsupported file type '.{ext}'. Please choose a .pdf, .fit, .txt, or .md file."
            ))
        }
    };
    let bytes = std::fs::read(src).map_err(|e| format!("Failed to read file: {e}"))?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err("Intake file exceeds 20 MB limit.".to_string());
    }
    let clean_name: String = orig_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let id = uuid::Uuid::new_v4().to_string();
    let rel_path = format!("attachments/{id}_{clean_name}");
    SharedStore::atomic_write_bytes(&store.root().join(&rel_path), &bytes)
        .map_err(|e| e.to_string())?;

    let mut flow = store.load_first_use_state().map_err(|e| e.to_string())?;
    flow.queued_intake_files.push(IntakeQueuedFile {
        id,
        file_name: orig_name,
        relative_path: rel_path,
        mime_type,
        kind,
        added_at: Utc::now().to_rfc3339(),
    });
    store.save_first_use_state(&flow).map_err(|e| e.to_string())?;
    store.load_first_use_state().map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_intake_file(
    state: State<'_, AppState>,
    file_id: String,
) -> Result<FirstUseFlowState, String> {
    let store = state.load_store()?;
    let mut flow = store.load_first_use_state().map_err(|e| e.to_string())?;
    if let Some(pos) = flow.queued_intake_files.iter().position(|f| f.id == file_id) {
        let removed = flow.queued_intake_files.remove(pos);
        let _ = store.remove_relative_file(&removed.relative_path);
        store.save_first_use_state(&flow).map_err(|e| e.to_string())?;
    }
    store.load_first_use_state().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_auto_calibrate_after_sync(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<FirstUseFlowState, String> {
    let store = state.load_store()?;
    let mut flow = store.load_first_use_state().map_err(|e| e.to_string())?;
    flow.auto_calibrate_after_first_sync = enabled;
    store.save_first_use_state(&flow).map_err(|e| e.to_string())?;
    store.load_first_use_state().map_err(|e| e.to_string())
}

#[tauri::command]
fn advance_first_use_stage(
    state: State<'_, AppState>,
    stage: FirstUseStage,
) -> Result<FirstUseFlowState, String> {
    let store = state.load_store()?;
    let mut flow = store.load_first_use_state().map_err(|e| e.to_string())?;
    flow.stage = stage;
    store.save_first_use_state(&flow).map_err(|e| e.to_string())?;
    store.load_first_use_state().map_err(|e| e.to_string())
}

#[tauri::command]
async fn complete_history_sync_stage(
    state: State<'_, AppState>,
    run_garmin_sync: bool,
) -> Result<serde_json::Value, String> {
    let store = state.load_store()?;
    let db = state.open_db()?;
    if run_garmin_sync {
        perform_full_garmin_sync(&store, &db).await?;
    }

    let mut flow = store.load_first_use_state().map_err(|e| e.to_string())?;
    let mut config = store.load_athlete_config().map_err(|e| e.to_string())?.unwrap_or_default();
    let mut user_profile = db.get_user_profile().map_err(|e| e.to_string())?;
    let activities = db.list_activities(5000).map_err(|e| e.to_string())?;
    let days = db.list_days(365).map_err(|e| e.to_string())?;

    let calibrated = if flow.auto_calibrate_after_first_sync {
        let changed = coach::auto_calibrate_thresholds_from_history(
            &mut config,
            &mut user_profile,
            &activities,
            &days,
            Utc::now(),
        );
        if changed {
            store.save_athlete_config(&config).map_err(|e| e.to_string())?;
            db.save_user_profile(&user_profile).map_err(|e| e.to_string())?;
        }
        changed
    } else {
        false
    };

    let summary = coach::analyze_training_history(&activities, &config, Utc::now());
    let md = coach::compact_history_markdown(&summary);
    flow.history_summary_markdown = Some(md.clone());
    flow.stage = FirstUseStage::PlanApproval;
    store.save_first_use_state(&flow).map_err(|e| e.to_string())?;
    let saved_flow = store.load_first_use_state().map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "first_use_state": saved_flow,
        "summary_markdown": md,
        "synced_activities": activities.len(),
        "calibrated": calibrated,
    }))
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
    let model_settings = db.load_model_settings().unwrap_or_default();
    let first_use_state = store.load_first_use_state().unwrap_or_default();

    if let Some(api_key) = CredentialStore::read_secret(APP_SERVICE, GEMINI_ACCOUNT) {
        let user_profile = db.get_user_profile().ok();
        let sys = coach::build_static_system_instruction(
            &store,
            &config,
            &activities,
            user_profile.as_ref(),
            Some(&first_use_state),
        );
        let intake_slice = if matches!(
            first_use_state.stage,
            FirstUseStage::Intake | FirstUseStage::ProfileApproval
        ) {
            first_use_state.queued_intake_files.as_slice()
        } else {
            &[]
        };
        let (text, proposals) = coach::run_gemini_coach_turn(
            Some(&app),
            &api_key,
            &sys,
            &prompt,
            &store,
            Some(&db),
            &activities,
            &days,
            &model_settings,
            intake_slice,
        )
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
        let packet = attach_latest_forensic(packet, &db, &config, &activities);
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
            verify_gemini_key,
            clean_slate_reset,
            probe_fit_baseline,
            save_gemini_key,
            save_global_model_settings,
            save_coach_model_override,
            save_athlete_config,
            save_user_profile,
            reclassify_activity_as_hyrox,
            set_execution_mode,
            set_custom_shared_dir,
            recompute_readiness,
            sync_garmin_now,
            refresh_activity_metadata,
            queue_intake_file,
            remove_intake_file,
            set_auto_calibrate_after_sync,
            advance_first_use_stage,
            complete_history_sync_stage,
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
