use crate::models::{
    ActivityRecord, AthleteConfig, ClassifiedLap, CompromisedRunAudit, DayRecord,
    FitBaselineProbe, FitGuardrailAudit, FitLap, FitSplit, FitTrackAnalysis, FitTrackPoint,
    HyroxDissection, LapKind, APP_SERVICE, GARMIN_ACCOUNT, GARMIN_KEYRING_SERVICE,
};
use crate::storage::CredentialStore;
use chrono::{Duration, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::time::{Duration as StdDuration, Instant};
use thiserror::Error;
use tokio::sync::Mutex;

pub const GARMIN_API_BASE: &str = "https://personal.connectapi.garmin.com";
const TOKEN_PATH: &str = "/personalapitoken-service/personal-api/personal-api-key/token";
const TOKEN_LIFETIME_SECS: i64 = 23 * 3600;
const REQUEST_GAP_MS: u64 = 350;

#[derive(Debug, Error)]
pub enum GarminError {
    #[error("Add your Personal API key first.")]
    NoKey,
    #[error("The key wasn't accepted. It may have expired or been deleted.")]
    BadKey,
    #[error("Rate limited by Garmin. Try again in a few minutes.")]
    RateLimited,
    #[error("HTTP error {0}: {1}")]
    Http(u16, String),
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("JSON decode error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ZIP unpack error: {0}")]
    Zip(String),
}

struct TokenState {
    value: String,
    expires_at: chrono::DateTime<Utc>,
}

pub struct GarminClient {
    http: Client,
    api_key: Option<String>,
    token: Mutex<Option<TokenState>>,
    last_request: Mutex<Option<Instant>>,
}

impl GarminClient {
    pub fn new(api_key_override: Option<String>) -> Self {
        Self {
            http: Client::new(),
            api_key: api_key_override,
            token: Mutex::new(None),
            last_request: Mutex::new(None),
        }
    }

    fn resolve_key(&self) -> Result<String, GarminError> {
        if let Some(ref k) = self.api_key {
            let trimmed = k.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
        CredentialStore::read_secret(GARMIN_KEYRING_SERVICE, GARMIN_ACCOUNT)
            .or_else(|| CredentialStore::read_secret(APP_SERVICE, GARMIN_ACCOUNT))
            .ok_or(GarminError::NoKey)
    }

    async fn pace_gap(&self) {
        let mut guard = self.last_request.lock().await;
        if let Some(prev) = *guard {
            let elapsed = prev.elapsed();
            let min_gap = StdDuration::from_millis(REQUEST_GAP_MS);
            if elapsed < min_gap {
                tokio::time::sleep(min_gap - elapsed).await;
            }
        }
        *guard = Some(Instant::now());
    }

    pub async fn authenticate(&self, force: bool) -> Result<String, GarminError> {
        {
            let guard = self.token.lock().await;
            if !force {
                if let Some(ref st) = *guard {
                    if st.expires_at > Utc::now() {
                        return Ok(st.value.clone());
                    }
                }
            }
        }

        let key = self.resolve_key()?;
        self.pace_gap().await;
        let url = format!("{GARMIN_API_BASE}{TOKEN_PATH}");
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("ApiKey {key}"))
            .send()
            .await?;

        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if status != 200 {
            if status == 401 || status == 403 {
                return Err(GarminError::BadKey);
            }
            return Err(GarminError::Http(status, body.chars().take(200).collect()));
        }

        let mut token_val = body.trim().to_string();
        if token_val.starts_with('{') {
            if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&token_val) {
                if let Some(t) = obj
                    .get("access_token")
                    .or_else(|| obj.get("accessToken"))
                    .or_else(|| obj.get("token"))
                    .and_then(|v| v.as_str())
                {
                    token_val = t.to_string();
                }
            }
        }

        let mut guard = self.token.lock().await;
        *guard = Some(TokenState {
            value: token_val.clone(),
            expires_at: Utc::now() + Duration::seconds(TOKEN_LIFETIME_SECS),
        });
        Ok(token_val)
    }

    pub async fn get_bytes(&self, path: &str, query: &[(&str, &str)], accept: &str) -> Result<Vec<u8>, GarminError> {
        let mut renewed = false;
        for attempt in 0..4u64 {
            let bearer = self.authenticate(false).await?;
            self.pace_gap().await;
            let url = format!("{GARMIN_API_BASE}{path}");
            let resp = self
                .http
                .get(&url)
                .query(query)
                .header("Authorization", format!("Bearer {bearer}"))
                .header("Accept", accept)
                .send()
                .await?;

            let status = resp.status().as_u16();
            match status {
                200 => return Ok(resp.bytes().await?.to_vec()),
                204 => return Ok(b"null".to_vec()),
                401 if !renewed => {
                    renewed = true;
                    let _ = self.authenticate(true).await?;
                }
                401 | 403 => return Err(GarminError::BadKey),
                429 => {
                    if attempt == 3 {
                        return Err(GarminError::RateLimited);
                    }
                    tokio::time::sleep(StdDuration::from_secs(15 * (attempt + 1))).await;
                }
                code => {
                    let text = resp.text().await.unwrap_or_default();
                    return Err(GarminError::Http(code, text.chars().take(200).collect()));
                }
            }
        }
        Err(GarminError::RateLimited)
    }

    pub async fn download_fit_bytes(&self, activity_id: i64) -> Result<Vec<u8>, GarminError> {
        let zip_bytes = self
            .get_bytes(&format!("/download-service/files/activity/{activity_id}"), &[], "*/*")
            .await?;
        let cursor = Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| GarminError::Zip(e.to_string()))?;
        let archive_len = archive.len();
        for i in 0..archive_len {
            let mut file = archive.by_index(i).map_err(|e| GarminError::Zip(e.to_string()))?;
            if file.name().to_lowercase().ends_with(".fit") || archive_len == 1 {
                let mut out = Vec::new();
                file.read_to_end(&mut out).map_err(|e| GarminError::Zip(e.to_string()))?;
                return Ok(out);
            }
        }
        Err(GarminError::Zip("No .fit file found in activity ZIP".to_string()))
    }

    pub async fn sync_recent(&self, days_back: i64) -> Result<(Vec<ActivityRecord>, Vec<DayRecord>), GarminError> {
        let now = Utc::now();
        let start_str = (now - Duration::days(days_back)).format("%Y-%m-%d").to_string();
        let end_str = now.format("%Y-%m-%d").to_string();

        // 1. Activities
        let act_bytes = self
            .get_bytes(
                "/activitylist-service/activities/search/activities",
                &[("start", "0"), ("limit", "100"), ("startDate", &start_str)],
                "application/json",
            )
            .await?;
        let dtos: Vec<ActivityDTO> = serde_json::from_slice(&act_bytes).unwrap_or_default();
        let mut activities = Vec::with_capacity(dtos.len());
        for dto in dtos {
            activities.push(ActivityRecord {
                garmin_id: dto.activity_id,
                name: dto.activity_name.unwrap_or_default(),
                type_key: dto.activity_type.and_then(|t| t.type_key).unwrap_or_else(|| "other".to_string()),
                start_iso: dto.start_time_local.unwrap_or_else(|| now.to_rfc3339()),
                distance_m: dto.distance.unwrap_or(0.0),
                duration_sec: dto.duration.unwrap_or(0.0),
                moving_duration_sec: dto.moving_duration.or(dto.duration).unwrap_or(0.0),
                elevation_gain_m: dto.elevation_gain.unwrap_or(0.0),
                average_hr: dto.average_hr,
                max_hr: dto.max_hr,
                cadence_spm: dto.average_running_cadence_in_steps_per_minute,
                training_load: dto.activity_training_load,
                shoe_name: None,
                is_hyrox: None,
            });
        }

        // 2. Daily summaries + Sleep + HRV
        let mut days_map: HashMap<String, DayRecord> = HashMap::new();

        if let Ok(sum_bytes) = self
            .get_bytes(
                "/usersummary-service/v2/dailysummaries",
                &[("startDate", &start_str), ("endDate", &end_str)],
                "application/json",
            )
            .await
        {
            if let Ok(summaries) = serde_json::from_slice::<DailySummariesDTO>(&sum_bytes) {
                for d in summaries.data {
                    if let Some(date) = d.calendar_date {
                        let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));
                        entry.resting_hr = d.heart_rate.and_then(|h| h.resting_value);
                        entry.stress_avg = d.stress.and_then(|s| s.avg_level);
                        if let Some(bb) = d.body_battery {
                            entry.body_battery_high = bb.max_value;
                            entry.body_battery_latest = bb.latest_value;
                        }
                        if let Some(steps) = d.movement.and_then(|m| m.steps) {
                            entry.steps = steps.count.or(steps.value);
                            entry.step_goal = steps.goal;
                        }
                    }
                }
            }
        }

        if let Ok(sleep_bytes) = self
            .get_bytes(
                "/sleep-service/sleep/dailySleepSummariesByDate",
                &[("startDate", &start_str), ("endDate", &end_str)],
                "application/json",
            )
            .await
        {
            if let Ok(sleeps) = serde_json::from_slice::<Vec<SleepDTO>>(&sleep_bytes) {
                for s in sleeps {
                    if let Some(date) = s.calendar_date {
                        let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));
                        entry.sleep_seconds = s.sleep_time_seconds;
                        entry.sleep_score = s.sleep_scores.and_then(|sc| sc.overall).and_then(|o| o.value);
                    }
                }
            }
        }

        if let Ok(hrv_bytes) = self
            .get_bytes(
                &format!("/hrv-service/hrv/daily/{start_str}/{end_str}"),
                &[],
                "application/json",
            )
            .await
        {
            if let Ok(hrv) = serde_json::from_slice::<HRVRangeDTO>(&hrv_bytes) {
                for h in hrv.hrv_summaries.unwrap_or_default() {
                    if let Some(date) = h.calendar_date {
                        let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));
                        entry.hrv_last_night = h.last_night_avg;
                        entry.hrv_status = h.status;
                    }
                }
            }
        }

        let days: Vec<DayRecord> = days_map.into_values().collect();
        Ok((activities, days))
    }

    /// Full lifetime activity pagination + 365-day wellness backfill on first sync,
    /// or incremental 35-day sync when `initial_backfill` is false.
    pub async fn sync_lifetime_and_wellness(
        &self,
        initial_backfill: bool,
    ) -> Result<(Vec<ActivityRecord>, Vec<DayRecord>), GarminError> {
        if !initial_backfill {
            return self.sync_recent(35).await;
        }

        let now = Utc::now();
        let mut activities = Vec::new();
        let mut start = 0usize;
        const PAGE_SIZE: usize = 100;
        const MAX_PAGES: usize = 50;

        for _ in 0..MAX_PAGES {
            let start_param = start.to_string();
            let limit_param = PAGE_SIZE.to_string();
            let act_bytes = self
                .get_bytes(
                    "/activitylist-service/activities/search/activities",
                    &[("start", &start_param), ("limit", &limit_param)],
                    "application/json",
                )
                .await?;
            let dtos: Vec<ActivityDTO> = serde_json::from_slice(&act_bytes).unwrap_or_default();
            let count = dtos.len();
            for dto in dtos {
                activities.push(ActivityRecord {
                    garmin_id: dto.activity_id,
                    name: dto.activity_name.unwrap_or_default(),
                    type_key: dto
                        .activity_type
                        .and_then(|t| t.type_key)
                        .unwrap_or_else(|| "other".to_string()),
                    start_iso: dto.start_time_local.unwrap_or_else(|| now.to_rfc3339()),
                    distance_m: dto.distance.unwrap_or(0.0),
                    duration_sec: dto.duration.unwrap_or(0.0),
                    moving_duration_sec: dto.moving_duration.or(dto.duration).unwrap_or(0.0),
                    elevation_gain_m: dto.elevation_gain.unwrap_or(0.0),
                    average_hr: dto.average_hr,
                    max_hr: dto.max_hr,
                    cadence_spm: dto.average_running_cadence_in_steps_per_minute,
                    training_load: dto.activity_training_load,
                    shoe_name: None,
                    is_hyrox: None,
                });
            }
            if count < PAGE_SIZE {
                break;
            }
            start += PAGE_SIZE;
        }

        // 365-day wellness backfill in 35-day chunks
        let mut days_map: HashMap<String, DayRecord> = HashMap::new();
        let mut chunk_end = now;
        let earliest = now - Duration::days(365);
        while chunk_end > earliest {
            let chunk_start = (chunk_end - Duration::days(35)).max(earliest);
            let s_str = chunk_start.format("%Y-%m-%d").to_string();
            let e_str = chunk_end.format("%Y-%m-%d").to_string();

            if let Ok(sum_bytes) = self
                .get_bytes(
                    "/usersummary-service/v2/dailysummaries",
                    &[("startDate", &s_str), ("endDate", &e_str)],
                    "application/json",
                )
                .await
            {
                if let Ok(summaries) = serde_json::from_slice::<DailySummariesDTO>(&sum_bytes) {
                    for d in summaries.data {
                        if let Some(date) = d.calendar_date {
                            let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));
                            entry.resting_hr = d.heart_rate.and_then(|h| h.resting_value);
                            entry.stress_avg = d.stress.and_then(|s| s.avg_level);
                            if let Some(bb) = d.body_battery {
                                entry.body_battery_high = bb.max_value;
                                entry.body_battery_latest = bb.latest_value;
                            }
                            if let Some(steps) = d.movement.and_then(|m| m.steps) {
                                entry.steps = steps.count.or(steps.value);
                                entry.step_goal = steps.goal;
                            }
                        }
                    }
                }
            }

            if let Ok(sleep_bytes) = self
                .get_bytes(
                    "/sleep-service/sleep/dailySleepSummariesByDate",
                    &[("startDate", &s_str), ("endDate", &e_str)],
                    "application/json",
                )
                .await
            {
                if let Ok(sleeps) = serde_json::from_slice::<Vec<SleepDTO>>(&sleep_bytes) {
                    for s in sleeps {
                        if let Some(date) = s.calendar_date {
                            let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));
                            entry.sleep_seconds = s.sleep_time_seconds;
                            entry.sleep_score = s.sleep_scores.and_then(|sc| sc.overall).and_then(|o| o.value);
                        }
                    }
                }
            }

            if let Ok(hrv_bytes) = self
                .get_bytes(
                    &format!("/hrv-service/hrv/daily/{s_str}/{e_str}"),
                    &[],
                    "application/json",
                )
                .await
            {
                if let Ok(hrv) = serde_json::from_slice::<HRVRangeDTO>(&hrv_bytes) {
                    for h in hrv.hrv_summaries.unwrap_or_default() {
                        if let Some(date) = h.calendar_date {
                            let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));
                            entry.hrv_last_night = h.last_night_avg;
                            entry.hrv_status = h.status;
                        }
                    }
                }
            }

            chunk_end = chunk_start - Duration::days(1);
        }

        let days: Vec<DayRecord> = days_map.into_values().collect();
        Ok((activities, days))
    }
}

fn empty_day(date: &str) -> DayRecord {
    DayRecord {
        date: date.to_string(),
        resting_hr: None,
        stress_avg: None,
        body_battery_high: None,
        body_battery_latest: None,
        steps: None,
        step_goal: None,
        sleep_seconds: None,
        sleep_score: None,
        hrv_last_night: None,
        hrv_status: None,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityDTO {
    activity_id: i64,
    activity_name: Option<String>,
    start_time_local: Option<String>,
    activity_type: Option<ActivityTypeDTO>,
    distance: Option<f64>,
    duration: Option<f64>,
    moving_duration: Option<f64>,
    elevation_gain: Option<f64>,
    #[serde(rename = "averageHR")]
    average_hr: Option<f64>,
    #[serde(rename = "maxHR")]
    max_hr: Option<f64>,
    average_running_cadence_in_steps_per_minute: Option<f64>,
    activity_training_load: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityTypeDTO {
    type_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DailySummariesDTO {
    data: Vec<DailySummaryDayDTO>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailySummaryDayDTO {
    calendar_date: Option<String>,
    heart_rate: Option<DailyHeartRateSummaryDTO>,
    stress: Option<DailyStressDTO>,
    body_battery: Option<DailyBodyBatteryDTO>,
    movement: Option<DailyMovementDTO>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyHeartRateSummaryDTO {
    resting_value: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyStressDTO {
    avg_level: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyBodyBatteryDTO {
    max_value: Option<i32>,
    latest_value: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct DailyMovementDTO {
    steps: Option<DailyStepsInnerDTO>,
}

#[derive(Debug, Deserialize)]
struct DailyStepsInnerDTO {
    count: Option<i32>,
    value: Option<i32>,
    goal: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SleepDTO {
    calendar_date: Option<String>,
    sleep_time_seconds: Option<i32>,
    sleep_scores: Option<SleepScoresDTO>,
}

#[derive(Debug, Deserialize)]
struct SleepScoresDTO {
    overall: Option<SleepScoreOverallDTO>,
}

#[derive(Debug, Deserialize)]
struct SleepScoreOverallDTO {
    value: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HRVRangeDTO {
    hrv_summaries: Option<Vec<HRVSummaryDTO>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HRVSummaryDTO {
    calendar_date: Option<String>,
    last_night_avg: Option<i32>,
    status: Option<String>,
}

// MARK: - .fit Track Decoding, Splits, Cardiac Decoupling & Guardrail Audit

fn value_as_f64(val: &fitparser::Value) -> Option<f64> {
    match val {
        fitparser::Value::Float32(v) => Some(*v as f64),
        fitparser::Value::Float64(v) => Some(*v),
        fitparser::Value::SInt8(v) => Some(*v as f64),
        fitparser::Value::UInt8(v) => Some(*v as f64),
        fitparser::Value::SInt16(v) => Some(*v as f64),
        fitparser::Value::UInt16(v) => Some(*v as f64),
        fitparser::Value::SInt32(v) => Some(*v as f64),
        fitparser::Value::UInt32(v) => Some(*v as f64),
        fitparser::Value::SInt64(v) => Some(*v as f64),
        fitparser::Value::UInt64(v) => Some(*v as f64),
        _ => None,
    }
}

pub fn parse_fit_track(
    garmin_id: i64,
    bytes: &[u8],
    config: &AthleteConfig,
) -> Result<FitTrackAnalysis, String> {
    let records = fitparser::from_bytes(bytes).map_err(|e| format!("FIT parse error: {e}"))?;
    let mut points = Vec::new();
    let mut laps = Vec::new();
    let mut base_ts: Option<f64> = None;

    for msg in records {
        if msg.kind() == fitparser::profile::MesgNum::Lap {
            let mut elapsed_sec = 0.0;
            let mut moving_sec = 0.0;
            let mut distance_m = 0.0;
            let mut avg_hr_bpm: Option<f64> = None;
            for field in msg.fields() {
                match field.name() {
                    "total_elapsed_time" => elapsed_sec = value_as_f64(field.value()).unwrap_or(0.0),
                    "total_timer_time" => moving_sec = value_as_f64(field.value()).unwrap_or(0.0),
                    "total_distance" => distance_m = value_as_f64(field.value()).unwrap_or(0.0),
                    "avg_heart_rate" => avg_hr_bpm = value_as_f64(field.value()),
                    _ => {}
                }
            }
            if elapsed_sec > 0.0 || moving_sec > 0.0 || distance_m > 0.0 {
                laps.push(FitLap {
                    elapsed_sec,
                    moving_sec: if moving_sec > 0.0 { moving_sec } else { elapsed_sec },
                    distance_m,
                    avg_hr_bpm,
                });
            }
            continue;
        }
        if msg.kind() != fitparser::profile::MesgNum::Record {
            continue;
        }
        let mut ts_sec: Option<f64> = None;
        let mut distance_m: Option<f64> = None;
        let mut speed_mps: Option<f64> = None;
        let mut hr: Option<f64> = None;
        let mut cadence: Option<f64> = None;
        let mut altitude: Option<f64> = None;

        for field in msg.fields() {
            match field.name() {
                "timestamp" => {
                    if let fitparser::Value::Timestamp(dt) = field.value() {
                        ts_sec = Some(dt.timestamp() as f64);
                    }
                }
                "distance" => distance_m = value_as_f64(field.value()),
                "enhanced_speed" | "speed" => {
                    if speed_mps.is_none() || field.name() == "enhanced_speed" {
                        speed_mps = value_as_f64(field.value());
                    }
                }
                "heart_rate" => hr = value_as_f64(field.value()),
                "cadence" => cadence = value_as_f64(field.value()).map(|c| c * 2.0),
                "enhanced_altitude" | "altitude" => {
                    if altitude.is_none() || field.name() == "enhanced_altitude" {
                        altitude = value_as_f64(field.value());
                    }
                }
                _ => {}
            }
        }

        let Some(dist) = distance_m else { continue };
        let abs_ts = ts_sec.unwrap_or(points.len() as f64);
        let origin = *base_ts.get_or_insert(abs_ts);
        let rel_ts = (abs_ts - origin).max(0.0);
        let spd = speed_mps.unwrap_or(0.0).max(0.0);
        let pace = if spd > 0.2 { Some(1000.0 / spd) } else { None };

        points.push(FitTrackPoint {
            timestamp_sec: rel_ts,
            distance_m: dist,
            speed_mps: spd,
            pace_sec_per_km: pace,
            heart_rate_bpm: hr,
            cadence_spm: cadence,
            altitude_m: altitude,
        });
    }

    let mut analysis = analyze_fit_points(garmin_id, points, config);
    analysis.compromised_run_audit = CompromisedRunAudit::compute(&laps);
    analysis.hyrox_dissection = dissect_hyrox_laps(&laps, false);
    analysis.laps = laps;
    Ok(analysis)
}

/// Scans a local folder of `.fit` files to pre-fill onboarding telemetry baselines:
/// `detected_max_hr`, `detected_rhr_baseline`, `estimated_lthr`, and `weekly_km_est`.
pub fn probe_fit_folder(folder_path: &std::path::Path) -> Result<FitBaselineProbe, String> {
    if !folder_path.exists() || !folder_path.is_dir() {
        return Err(format!("Folder does not exist: {}", folder_path.display()));
    }
    let mut fit_paths: Vec<std::path::PathBuf> = std::fs::read_dir(folder_path)
        .map_err(|e| format!("Failed to read folder: {e}"))?
        .filter_map(|e| e.ok().map(|x| x.path()))
        .filter(|p| {
            p.extension()
                .and_then(|s| s.to_str())
                .map_or(false, |ext| ext.eq_ignore_ascii_case("fit"))
        })
        .collect();
    fit_paths.sort();
    fit_paths.reverse();
    fit_paths.truncate(30);

    let mut max_hr_seen: Option<f64> = None;
    let mut min_hr_seen: Option<f64> = None;
    let mut total_dist_m = 0.0_f64;
    let mut min_ts: Option<i64> = None;
    let mut max_ts: Option<i64> = None;
    let mut scanned = 0usize;

    for path in &fit_paths {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let Ok(records) = fitparser::from_bytes(&bytes) else { continue };
        scanned += 1;

        let mut session_dist = 0.0_f64;
        let mut rec_max_dist = 0.0_f64;
        for msg in records {
            if msg.kind() == fitparser::profile::MesgNum::Session {
                for field in msg.fields() {
                    match field.name() {
                        "total_distance" => {
                            if let Some(d) = value_as_f64(field.value()) {
                                session_dist = session_dist.max(d);
                            }
                        }
                        "max_heart_rate" => {
                            if let Some(hr) = value_as_f64(field.value()) {
                                if hr > 80.0 && hr < 230.0 {
                                    max_hr_seen = Some(max_hr_seen.map_or(hr, |m| m.max(hr)));
                                }
                            }
                        }
                        "min_heart_rate" => {
                            if let Some(hr) = value_as_f64(field.value()) {
                                if hr > 30.0 && hr < 120.0 {
                                    min_hr_seen = Some(min_hr_seen.map_or(hr, |m| m.min(hr)));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            } else if msg.kind() == fitparser::profile::MesgNum::Record {
                for field in msg.fields() {
                    match field.name() {
                        "heart_rate" => {
                            if let Some(hr) = value_as_f64(field.value()) {
                                if hr > 80.0 && hr < 230.0 {
                                    max_hr_seen = Some(max_hr_seen.map_or(hr, |m| m.max(hr)));
                                }
                            }
                        }
                        "distance" => {
                            if let Some(d) = value_as_f64(field.value()) {
                                rec_max_dist = rec_max_dist.max(d);
                            }
                        }
                        "timestamp" => {
                            if let fitparser::Value::Timestamp(dt) = field.value() {
                                let ts = dt.timestamp();
                                min_ts = Some(min_ts.map_or(ts, |m| m.min(ts)));
                                max_ts = Some(max_ts.map_or(ts, |m| m.max(ts)));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        total_dist_m += if session_dist > 0.0 { session_dist } else { rec_max_dist };
    }

    let detected_max_hr = max_hr_seen.map(|v| v.round() as i32);
    let estimated_lthr = max_hr_seen.map(|v| (v * 0.89).round() as i32);
    let detected_rhr_baseline = min_hr_seen.map(|v| v.round() as i32);
    let weekly_km_est = if total_dist_m > 0.0 {
        let span_days = match (min_ts, max_ts) {
            (Some(a), Some(b)) if b > a => ((b - a) as f64 / 86_400.0).max(7.0),
            _ => 28.0,
        };
        let weeks = (span_days / 7.0).max(1.0);
        Some(((total_dist_m / 1000.0) / weeks * 10.0).round() / 10.0)
    } else {
        None
    };

    Ok(FitBaselineProbe {
        scanned_files: scanned,
        detected_max_hr,
        detected_rhr_baseline,
        estimated_lthr,
        weekly_km_est,
    })
}

/// Classifies `.fit` laps into `Run`, `Station`, and `Roxzone` transitions and computes
/// clean running metrics (`clean_run_pace_sec_per_km`, `clean_aerobic_efficiency`) plus
/// HYROX compromised-run decay (`compromised_pace_decay_sec`) and recovery duration.
pub fn dissect_hyrox_laps(laps: &[FitLap], force_hyrox: bool) -> Option<HyroxDissection> {
    if laps.is_empty() {
        return None;
    }

    let mut run_splits = Vec::new();
    let mut station_splits = Vec::new();
    let mut roxzone_transitions = Vec::new();
    let mut seq_kinds = Vec::new();

    for (idx, lap) in laps.iter().enumerate() {
        let moving = if lap.moving_sec > 0.0 {
            lap.moving_sec
        } else {
            lap.elapsed_sec
        };
        if moving <= 0.0 {
            continue;
        }
        let pace = if lap.distance_m > 0.0 {
            Some(moving / (lap.distance_m / 1000.0))
        } else {
            None
        };

        let is_run = lap.distance_m >= 600.0 && pace.map_or(false, |p| p <= 600.0);
        let is_roxzone = !is_run
            && ((lap.distance_m > 0.0 && lap.distance_m < 350.0 && moving <= 75.0)
                || (lap.distance_m < 600.0 && moving <= 60.0));

        let kind = if is_run {
            LapKind::Run
        } else if is_roxzone {
            LapKind::Roxzone
        } else {
            LapKind::Station
        };

        let classified = ClassifiedLap {
            lap_index: idx + 1,
            kind,
            elapsed_sec: lap.elapsed_sec,
            moving_sec: moving,
            distance_m: lap.distance_m,
            pace_sec_per_km: pace.map(|p| (p * 10.0).round() / 10.0),
            avg_hr_bpm: lap.avg_hr_bpm,
        };

        seq_kinds.push(kind);
        match kind {
            LapKind::Run => run_splits.push(classified),
            LapKind::Station => station_splits.push(classified),
            LapKind::Roxzone => roxzone_transitions.push(classified),
        }
    }

    let mut run_nonrun_transitions = 0usize;
    for i in 1..seq_kinds.len() {
        let a_run = seq_kinds[i - 1] == LapKind::Run;
        let b_run = seq_kinds[i] == LapKind::Run;
        if a_run != b_run {
            run_nonrun_transitions += 1;
        }
    }

    let non_run_count = station_splits.len() + roxzone_transitions.len();
    let auto_detected = (run_splits.len() >= 2 && non_run_count >= 2 && run_nonrun_transitions >= 3)
        || (run_splits.len() >= 4 && station_splits.len() >= 2);
    let is_hyrox_sim = force_hyrox || auto_detected;

    if !is_hyrox_sim {
        return None;
    }

    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;

    let total_run_m: f64 = run_splits.iter().map(|s| s.distance_m).sum();
    let total_run_sec: f64 = run_splits.iter().map(|s| s.moving_sec).sum();
    let clean_run_pace_sec_per_km = if total_run_m > 0.0 && total_run_sec > 0.0 {
        Some(r1(total_run_sec / (total_run_m / 1000.0)))
    } else {
        None
    };

    let run_hrs: Vec<f64> = run_splits.iter().filter_map(|s| s.avg_hr_bpm).collect();
    let clean_aerobic_efficiency = if total_run_m > 0.0 && total_run_sec > 0.0 && !run_hrs.is_empty() {
        let speed_m_per_min = total_run_m / (total_run_sec / 60.0);
        let avg_hr = run_hrs.iter().sum::<f64>() / run_hrs.len() as f64;
        if avg_hr > 0.0 {
            Some(r3(speed_m_per_min / avg_hr))
        } else {
            None
        }
    } else {
        None
    };

    // Compromised pace decay: Runs 5-8 avg pace minus Run 1 pace (or second-half avg minus Run 1 when < 6 runs)
    let compromised_pace_decay_sec = if run_splits.len() >= 2 {
        let run1_pace = run_splits[0].pace_sec_per_km;
        let late_slice = if run_splits.len() >= 6 {
            &run_splits[4..run_splits.len().min(8)]
        } else {
            &run_splits[run_splits.len() / 2..]
        };
        let late_m: f64 = late_slice.iter().map(|s| s.distance_m).sum();
        let late_s: f64 = late_slice.iter().map(|s| s.moving_sec).sum();
        if let Some(p1) = run1_pace {
            if late_m > 0.0 && late_s > 0.0 {
                let late_pace = late_s / (late_m / 1000.0);
                Some(r1(late_pace - p1))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let station_hr_recovery_sec = if !roxzone_transitions.is_empty() {
        let avg = roxzone_transitions.iter().map(|s| s.moving_sec).sum::<f64>()
            / roxzone_transitions.len() as f64;
        Some(r1(avg))
    } else if !station_splits.is_empty() {
        let avg = station_splits.iter().map(|s| s.moving_sec).sum::<f64>()
            / station_splits.len() as f64;
        Some(r1(avg))
    } else {
        None
    };

    Some(HyroxDissection {
        is_hyrox_sim,
        run_splits,
        station_splits,
        roxzone_transitions,
        clean_run_pace_sec_per_km,
        clean_aerobic_efficiency,
        compromised_pace_decay_sec,
        station_hr_recovery_sec,
    })
}

pub fn analyze_fit_points(
    garmin_id: i64,
    full_points: Vec<FitTrackPoint>,
    config: &AthleteConfig,
) -> FitTrackAnalysis {
    let chart_points = downsample_points(&full_points, 240);
    let splits = compute_km_splits(&full_points);
    let decoupling_pct = compute_decoupling_pct(&full_points);

    let pace_floor = if config.medical.enforce_pace_floor {
        config.medical.min_pace_sec_per_km
    } else {
        None
    };
    let hr_ceiling = if config.medical.enforce_easy_hr_ceiling {
        config.medical.easy_hr_ceiling_bpm.or(config.specs.vt1_ceiling_bpm)
    } else {
        config.specs.vt1_ceiling_bpm
    };

    let mut pace_floor_violations = 0;
    let mut hr_ceiling_violations = 0;
    for s in &splits {
        if let (Some(floor_sec), Some(pace_sec)) = (pace_floor, s.pace_sec_per_km) {
            if pace_sec < floor_sec as f64 {
                pace_floor_violations += 1;
            }
        }
        if let (Some(ceil_bpm), Some(avg_hr)) = (hr_ceiling, s.avg_hr_bpm) {
            if avg_hr > ceil_bpm as f64 {
                hr_ceiling_violations += 1;
            }
        }
    }

    FitTrackAnalysis {
        garmin_id,
        chart_points,
        full_points,
        splits,
        laps: Vec::new(),
        compromised_run_audit: None,
        hyrox_dissection: None,
        decoupling_pct,
        audit: FitGuardrailAudit {
            pace_floor_sec_per_km: pace_floor,
            easy_hr_ceiling_bpm: hr_ceiling,
            pace_floor_violations,
            hr_ceiling_violations,
            compliant: pace_floor_violations == 0 && hr_ceiling_violations == 0,
        },
    }
}

fn downsample_points(points: &[FitTrackPoint], max_points: usize) -> Vec<FitTrackPoint> {
    if points.len() <= max_points || max_points == 0 {
        return points.to_vec();
    }
    let step = (points.len() as f64 / max_points as f64).ceil() as usize;
    points.iter().step_by(step.max(1)).cloned().collect()
}

fn compute_km_splits(points: &[FitTrackPoint]) -> Vec<FitSplit> {
    if points.len() < 2 {
        return Vec::new();
    }
    let mut splits = Vec::new();
    let mut bucket_start = 0usize;
    let mut km_index = 1i32;

    for i in 1..points.len() {
        let target_m = (km_index as f64) * 1000.0;
        if points[i].distance_m >= target_m || i == points.len() - 1 {
            let slice = &points[bucket_start..=i];
            let d = (slice.last().unwrap().distance_m - slice.first().unwrap().distance_m).max(0.0);
            let t = (slice.last().unwrap().timestamp_sec - slice.first().unwrap().timestamp_sec).max(0.0);
            if d >= 100.0 && t > 0.0 {
                let pace = Some(t / (d / 1000.0));
                let hrs: Vec<f64> = slice.iter().filter_map(|p| p.heart_rate_bpm).collect();
                let cads: Vec<f64> = slice.iter().filter_map(|p| p.cadence_spm).collect();
                let avg_hr = if hrs.is_empty() { None } else { Some(hrs.iter().sum::<f64>() / hrs.len() as f64) };
                let avg_cad = if cads.is_empty() { None } else { Some(cads.iter().sum::<f64>() / cads.len() as f64) };
                let mut elev_gain = 0.0;
                for w in slice.windows(2) {
                    if let (Some(a0), Some(a1)) = (w[0].altitude_m, w[1].altitude_m) {
                        if a1 > a0 {
                            elev_gain += a1 - a0;
                        }
                    }
                }
                splits.push(FitSplit {
                    km_index,
                    distance_m: d,
                    duration_sec: t,
                    pace_sec_per_km: pace,
                    avg_hr_bpm: avg_hr,
                    avg_cadence_spm: avg_cad,
                    elevation_gain_m: (elev_gain * 10.0).rounded_1(),
                });
                km_index += 1;
                bucket_start = i;
            }
        }
    }
    splits
}

trait RoundExt {
    fn rounded_1(self) -> f64;
}
impl RoundExt for f64 {
    fn rounded_1(self) -> f64 {
        (self * 10.0).round() / 10.0
    }
}

pub fn compute_decoupling_pct(points: &[FitTrackPoint]) -> Option<f64> {
    let valid: Vec<f64> = points
        .iter()
        .filter_map(|p| {
            let hr = p.heart_rate_bpm?;
            if p.speed_mps > 0.3 && hr > 40.0 {
                Some(p.speed_mps / hr)
            } else {
                None
            }
        })
        .collect();
    if valid.len() < 4 {
        return None;
    }
    let mid = valid.len() / 2;
    let ef1 = valid[..mid].iter().sum::<f64>() / mid as f64;
    let ef2 = valid[mid..].iter().sum::<f64>() / (valid.len() - mid) as f64;
    if ef1 <= 0.0 {
        return None;
    }
    Some((((ef1 - ef2) / ef1) * 100.0).rounded_1())
}
