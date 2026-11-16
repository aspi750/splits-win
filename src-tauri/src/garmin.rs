use crate::models::{ActivityRecord, DayRecord, APP_SERVICE, GARMIN_ACCOUNT};
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
        CredentialStore::read_secret(APP_SERVICE, GARMIN_ACCOUNT).ok_or(GarminError::NoKey)
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
