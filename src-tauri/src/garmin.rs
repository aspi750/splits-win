use crate::models::{
    default_temp_source, ActivityRecord, AthleteConfig, BodyBatteryPoint, ClassifiedLap,
    CompromisedRunAudit, DayRecord, FitBaselineProbe, FitGuardrailAudit, FitLap, FitSplit,
    FitTrackAnalysis, FitTrackPoint, HyroxDissection, LapKind, NightHrvDetail, NightHrvReading,
    WeightEntry, APP_SERVICE, GARMIN_ACCOUNT, GARMIN_KEYRING_SERVICE,
};
use crate::storage::CredentialStore;
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::time::{Duration as StdDuration, Instant};
use thiserror::Error;
use tokio::sync::Mutex;

pub const WELLNESS_CHUNK_DAYS: i64 = 28;

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
        self.get_bytes_inner(path, query, accept, true).await
    }

    async fn get_bytes_inner(
        &self,
        path: &str,
        query: &[(&str, &str)],
        accept: &str,
        retry_429: bool,
    ) -> Result<Vec<u8>, GarminError> {
        let mut renewed = false;
        let max_attempts = if retry_429 { 4u64 } else { 2u64 };
        for attempt in 0..max_attempts {
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
                    if !retry_429 || attempt + 1 >= max_attempts {
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
        Self::unpack_fit_zip(zip_bytes)
    }

    pub async fn download_fit_bytes_fast_429(&self, activity_id: i64) -> Result<Vec<u8>, GarminError> {
        let zip_bytes = self
            .get_bytes_inner(
                &format!("/download-service/files/activity/{activity_id}"),
                &[],
                "*/*",
                false,
            )
            .await?;
        Self::unpack_fit_zip(zip_bytes)
    }

    fn unpack_fit_zip(zip_bytes: Vec<u8>) -> Result<Vec<u8>, GarminError> {
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
        let now_utc = Utc::now();
        let end_date = Local::now().date_naive().max(now_utc.date_naive());
        let start_date = end_date - Duration::days(days_back.max(1));
        let start_str = start_date.format("%Y-%m-%d").to_string();

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
                start_iso: dto.start_time_local.unwrap_or_else(|| now_utc.to_rfc3339()),
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
                post_run_context: None,
                fit_temp_source: None,
                garmin_notes: normalize_optional_text(dto.description),
                is_pr: dto.is_pr,
                start_latitude: dto.start_latitude,
                start_longitude: dto.start_longitude,
            });
        }

        // 2. Daily summaries + Sleep + HRV + Steps + Body Battery (chunked <= 28 days per call)
        let days = self.sync_wellness_range(start_date, end_date).await;
        Ok((activities, days))
    }

    /// Full lifetime activity pagination + 365-day wellness backfill on first sync,
    /// or incremental 35-day sync when `initial_backfill` is false.
    pub async fn sync_lifetime_and_wellness(
        &self,
        initial_backfill: bool,
    ) -> Result<(Vec<ActivityRecord>, Vec<DayRecord>), GarminError> {
        self.sync_lifetime_and_wellness_with_progress(initial_backfill, |_, _, _, _| {})
            .await
    }

    pub async fn sync_lifetime_and_wellness_with_progress<F>(
        &self,
        initial_backfill: bool,
        mut on_progress: F,
    ) -> Result<(Vec<ActivityRecord>, Vec<DayRecord>), GarminError>
    where
        F: FnMut(&str, usize, usize, String),
    {
        if !initial_backfill {
            on_progress(
                "fetching",
                1,
                0,
                "Fetching recent Garmin activities & wellness…".to_string(),
            );
            let res = self.sync_recent(35).await?;
            on_progress(
                "fetching",
                1,
                res.0.len(),
                format!("Fetched {} recent activities…", res.0.len()),
            );
            return Ok(res);
        }

        let now_utc = Utc::now();
        let mut activities = Vec::new();
        let mut start = 0usize;
        const PAGE_SIZE: usize = 100;
        const MAX_PAGES: usize = 100;

        for page_idx in 0..MAX_PAGES {
            let page_num = page_idx + 1;
            let start_param = start.to_string();
            let limit_param = PAGE_SIZE.to_string();
            on_progress(
                "fetching",
                page_num,
                activities.len(),
                format!(
                    "Fetching Garmin history page {page_num} ({} activities)…",
                    activities.len()
                ),
            );
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
                    start_iso: dto.start_time_local.unwrap_or_else(|| now_utc.to_rfc3339()),
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
                    post_run_context: None,
                    fit_temp_source: None,
                    garmin_notes: normalize_optional_text(dto.description),
                    is_pr: dto.is_pr,
                    start_latitude: dto.start_latitude,
                    start_longitude: dto.start_longitude,
                });
            }
            on_progress(
                "fetching",
                page_num,
                activities.len(),
                format!(
                    "Fetching Garmin history page {page_num} ({} activities)…",
                    activities.len()
                ),
            );
            if count < PAGE_SIZE {
                break;
            }
            start += PAGE_SIZE;
        }

        // 365-day wellness backfill in 28-day chunks (Garmin Personal API limit)
        on_progress(
            "fetching",
            (activities.len() / PAGE_SIZE).max(1),
            activities.len(),
            format!(
                "Fetching 365-day wellness & sleep history ({} activities fetched)…",
                activities.len()
            ),
        );
        let end_date = Local::now().date_naive().max(now_utc.date_naive());
        let earliest = end_date - Duration::days(364);
        let days = self.sync_wellness_range(earliest, end_date).await;
        Ok((activities, days))
    }

    async fn sync_wellness_range(&self, earliest: NaiveDate, end_date: NaiveDate) -> Vec<DayRecord> {
        let mut days_map: HashMap<String, DayRecord> = HashMap::new();
        for (chunk_start, chunk_end) in chunk_date_range(earliest, end_date, WELLNESS_CHUNK_DAYS) {
            let s_str = chunk_start.format("%Y-%m-%d").to_string();
            let e_str = chunk_end.format("%Y-%m-%d").to_string();

            match self
                .get_bytes(
                    "/usersummary-service/v2/dailysummaries",
                    &[("startDate", &s_str), ("endDate", &e_str)],
                    "application/json",
                )
                .await
            {
                Ok(bytes) => parse_daily_summaries_json(&bytes, &mut days_map),
                Err(err) => eprintln!("Garmin dailysummaries skipped ({s_str}..{e_str}): {err}"),
            }

            match self
                .get_bytes(
                    "/sleep-service/sleep/dailySleepSummariesByDate",
                    &[("startDate", &s_str), ("endDate", &e_str)],
                    "application/json",
                )
                .await
            {
                Ok(bytes) => parse_sleep_summaries_json(&bytes, &mut days_map),
                Err(err) => eprintln!("Garmin sleep skipped ({s_str}..{e_str}): {err}"),
            }

            match self
                .get_bytes(
                    &format!("/hrv-service/hrv/daily/{s_str}/{e_str}"),
                    &[],
                    "application/json",
                )
                .await
            {
                Ok(bytes) => parse_hrv_range_json(&bytes, &mut days_map),
                Err(err) => eprintln!("Garmin HRV skipped ({s_str}..{e_str}): {err}"),
            }

            match self
                .get_bytes(
                    &format!("/usersummary-service/stats/steps/daily/{s_str}/{e_str}"),
                    &[],
                    "application/json",
                )
                .await
            {
                Ok(bytes) => parse_steps_json(&bytes, &mut days_map),
                Err(err) => eprintln!("Garmin steps skipped ({s_str}..{e_str}): {err}"),
            }
        }

        // Intraday Body Battery curve for the most recent 3 days (96 chunks/day ~ every 15 min / 6 min)
        let bb_start = (end_date - Duration::days(2)).max(earliest);
        let bb_s_str = bb_start.format("%Y-%m-%d").to_string();
        let bb_e_str = end_date.format("%Y-%m-%d").to_string();
        match self
            .get_bytes(
                "/wellness-service/wellness/bodyBattery/reports/daily",
                &[
                    ("startDate", &bb_s_str),
                    ("endDate", &bb_e_str),
                    ("chunks", "96"),
                ],
                "application/json",
            )
            .await
        {
            Ok(bytes) => parse_body_battery_days_json(&bytes, &mut days_map),
            Err(err) => eprintln!("Garmin bodyBattery intraday skipped ({bb_s_str}..{bb_e_str}): {err}"),
        }

        let mut days: Vec<DayRecord> = days_map.into_values().collect();
        days.sort_by(|a, b| b.date.cmp(&a.date));
        days
    }

    pub async fn fetch_today_body_battery(&self) -> Result<Option<DayRecord>, GarminError> {
        let today = Local::now().date_naive();
        let today_str = today.format("%Y-%m-%d").to_string();
        let mut days_map: HashMap<String, DayRecord> = HashMap::new();

        let bb_bytes = self
            .get_bytes(
                "/wellness-service/wellness/bodyBattery/reports/daily",
                &[
                    ("startDate", &today_str),
                    ("endDate", &today_str),
                    ("chunks", "96"),
                ],
                "application/json",
            )
            .await?;
        parse_body_battery_days_json(&bb_bytes, &mut days_map);

        if let Ok(sum_bytes) = self
            .get_bytes(
                "/usersummary-service/v2/dailysummaries",
                &[("startDate", &today_str), ("endDate", &today_str)],
                "application/json",
            )
            .await
        {
            parse_daily_summaries_json(&sum_bytes, &mut days_map);
        }

        Ok(days_map.remove(&today_str))
    }

    /// Intraday 5-min overnight HRV readings for `date` (`GET /hrv-service/hrv/{date}`).
    pub async fn fetch_hrv_night(&self, date: &str) -> Result<NightHrvDetail, GarminError> {
        let bytes = self
            .get_bytes(&format!("/hrv-service/hrv/{date}"), &[], "application/json")
            .await?;
        Ok(parse_hrv_night_json(date, &bytes))
    }

    /// Lightweight metadata-only fetch for a single activity (`/activity-service/activity/{activityId}`)
    /// without re-downloading its `.fit` binary.
    pub async fn fetch_activity_metadata(&self, activity_id: i64) -> Result<ActivityMetadataUpdate, GarminError> {
        let bytes = self
            .get_bytes(
                &format!("/activity-service/activity/{activity_id}"),
                &[],
                "application/json",
            )
            .await?;
        let detail: ActivityDetailDTO = serde_json::from_slice(&bytes)
            .unwrap_or(ActivityDetailDTO {
                activity_name: None,
                description: None,
                metadata_dto: None,
                summary_dto: None,
            });
        let name = normalize_optional_text(
            detail.activity_name.or_else(|| {
                detail
                    .summary_dto
                    .as_ref()
                    .and_then(|v| v.get("activityName"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            }),
        );
        let notes = normalize_optional_text(
            detail
                .description
                .or_else(|| {
                    detail
                        .metadata_dto
                        .as_ref()
                        .and_then(|v| v.get("description"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .or_else(|| {
                    detail
                        .summary_dto
                        .as_ref()
                        .and_then(|v| v.get("description"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                }),
        );
        Ok(ActivityMetadataUpdate {
            garmin_id: activity_id,
            name,
            garmin_notes: notes,
        })
    }

    /// Metadata-only search over the rolling window (minimum 7 days) to capture notes/titles edited
    /// in Garmin Connect after initial `.FIT` upload.
    pub async fn fetch_recent_activity_metadata(
        &self,
        days_back: i64,
    ) -> Result<Vec<ActivityMetadataUpdate>, GarminError> {
        let start_str = (Utc::now() - Duration::days(days_back.max(7)))
            .format("%Y-%m-%d")
            .to_string();
        let bytes = self
            .get_bytes(
                "/activitylist-service/activities/search/activities",
                &[("start", "0"), ("limit", "100"), ("startDate", &start_str)],
                "application/json",
            )
            .await?;
        let dtos: Vec<ActivityDTO> = serde_json::from_slice(&bytes).unwrap_or_default();
        Ok(dtos
            .into_iter()
            .map(|dto| ActivityMetadataUpdate {
                garmin_id: dto.activity_id,
                name: normalize_optional_text(dto.activity_name),
                garmin_notes: normalize_optional_text(dto.description),
            })
            .collect())
    }

    pub async fn sync_weights(&self, days_back: i64) -> Vec<WeightEntry> {
        let now = Utc::now();
        let mut by_epoch: HashMap<i64, WeightEntry> = HashMap::new();
        let mut chunk_end = now;
        let earliest = now - Duration::days(days_back.max(1));

        while chunk_end > earliest {
            let chunk_start = (chunk_end - Duration::days(35)).max(earliest);
            let s_str = chunk_start.format("%Y-%m-%d").to_string();
            let e_str = chunk_end.format("%Y-%m-%d").to_string();
            let path = format!("/weight-service/weight/range/{s_str}/{e_str}");
            if let Ok(bytes) = self
                .get_bytes(&path, &[("includeAll", "true")], "application/json")
                .await
            {
                for entry in parse_weight_range_json(&bytes) {
                    by_epoch.insert(entry.timestamp_epoch_sec, entry);
                }
            }
            chunk_end = chunk_start - Duration::days(1);
        }

        let mut out: Vec<WeightEntry> = by_epoch.into_values().collect();
        out.sort_by(|a, b| b.timestamp_epoch_sec.cmp(&a.timestamp_epoch_sec));
        out
    }
}

fn parse_weight_timestamp_epoch(metric: &serde_json::Value) -> Option<i64> {
    if let Some(ts) = metric.get("timestampGMT") {
        if let Some(n) = ts.as_f64().filter(|&v| v > 0.0) {
            return Some(if n > 10_000_000_000.0 {
                (n / 1000.0).round() as i64
            } else {
                n.round() as i64
            });
        }
        if let Some(s) = ts.as_str() {
            let trimmed = s.trim();
            if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
                return Some(dt.with_timezone(&Utc).timestamp());
            }
            let prefix: String = trimmed.chars().take(19).collect();
            for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
                if let Ok(ndt) = NaiveDateTime::parse_from_str(&prefix, fmt) {
                    return Some(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc).timestamp());
                }
            }
        }
    }
    if let Some(n) = metric.get("date").and_then(|v| v.as_f64()).filter(|&v| v > 0.0) {
        return Some(if n > 10_000_000_000.0 {
            (n / 1000.0).round() as i64
        } else {
            n.round() as i64
        });
    }
    if let Some(cal) = metric.get("calendarDate").and_then(|v| v.as_str()) {
        if let Ok(nd) = NaiveDate::parse_from_str(cal.trim(), "%Y-%m-%d") {
            if let Some(ndt) = nd.and_hms_opt(7, 0, 0) {
                if let Some(local_dt) = Local.from_local_datetime(&ndt).single() {
                    return Some(local_dt.with_timezone(&Utc).timestamp());
                }
                return Some(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc).timestamp());
            }
        }
    }
    None
}

fn normalize_mass_kg(raw: Option<f64>) -> Option<f64> {
    let v = raw.filter(|&x| x > 0.0)?;
    let kg = if v > 500.0 { v / 1000.0 } else { v };
    Some((kg * 100.0).round() / 100.0)
}

pub fn parse_weight_range_json(bytes: &[u8]) -> Vec<WeightEntry> {
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Vec::new();
    };
    let mut raw_metrics = Vec::new();
    if let Some(summaries) = val.get("dailyWeightSummaries").and_then(|v| v.as_array()) {
        for s in summaries {
            if let Some(all) = s.get("allWeightMetrics").and_then(|v| v.as_array()) {
                if !all.is_empty() {
                    raw_metrics.extend(all.iter().cloned());
                    continue;
                }
            }
            if let Some(latest) = s.get("latestWeight") {
                if !latest.is_null() {
                    raw_metrics.push(latest.clone());
                }
            }
        }
    }
    if let Some(list) = val.get("dateWeightList").and_then(|v| v.as_array()) {
        raw_metrics.extend(list.iter().cloned());
    }

    let mut by_epoch: HashMap<i64, WeightEntry> = HashMap::new();
    for m in raw_metrics {
        let Some(weight_kg) = normalize_mass_kg(m.get("weight").and_then(|v| v.as_f64())) else {
            continue;
        };
        let Some(epoch) = parse_weight_timestamp_epoch(&m) else {
            continue;
        };
        let iso = DateTime::<Utc>::from_timestamp(epoch, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| epoch.to_string());
        let is_manual = m
            .get("sourceType")
            .and_then(|v| v.as_str())
            .map(|s| s.to_uppercase().contains("MANUAL"))
            .unwrap_or(false);
        let prev = by_epoch.get(&epoch);
        let merged_source = if is_manual {
            prev.map(|p| p.source.clone()).unwrap_or_else(|| "manual".to_string())
        } else {
            "garmin_scale".to_string()
        };
        by_epoch.insert(
            epoch,
            WeightEntry {
                timestamp_epoch_sec: epoch,
                timestamp_iso: iso,
                weight_kg,
                body_fat_pct: m
                    .get("bodyFat")
                    .and_then(|v| v.as_f64())
                    .map(|v| (v * 10.0).round() / 10.0)
                    .or_else(|| prev.and_then(|p| p.body_fat_pct)),
                body_water_pct: m
                    .get("bodyWater")
                    .and_then(|v| v.as_f64())
                    .map(|v| (v * 10.0).round() / 10.0)
                    .or_else(|| prev.and_then(|p| p.body_water_pct)),
                muscle_mass_kg: normalize_mass_kg(m.get("muscleMass").and_then(|v| v.as_f64()))
                    .or_else(|| prev.and_then(|p| p.muscle_mass_kg)),
                source: merged_source,
            },
        );
    }

    let mut entries: Vec<WeightEntry> = by_epoch.into_values().collect();
    entries.sort_by(|a, b| b.timestamp_epoch_sec.cmp(&a.timestamp_epoch_sec));
    entries
}

pub fn chunk_date_range(
    start: NaiveDate,
    end: NaiveDate,
    max_days_inclusive: i64,
) -> Vec<(NaiveDate, NaiveDate)> {
    if end < start {
        return Vec::new();
    }
    let span = max_days_inclusive.max(1);
    let mut chunks = Vec::new();
    let mut chunk_end = end;
    while chunk_end >= start {
        let chunk_start = (chunk_end - Duration::days(span - 1)).max(start);
        chunks.push((chunk_start, chunk_end));
        if chunk_start <= start {
            break;
        }
        chunk_end = chunk_start - Duration::days(1);
    }
    chunks
}

fn json_opt_i32(val: Option<&serde_json::Value>) -> Option<i32> {
    let v = val?;
    if v.is_null() {
        return None;
    }
    if let Some(n) = v.as_i64() {
        return i32::try_from(n).ok();
    }
    if let Some(n) = v.as_u64() {
        return i32::try_from(n).ok();
    }
    if let Some(f) = v.as_f64().filter(|x| x.is_finite()) {
        return Some(f.round() as i32);
    }
    if let Some(s) = v.as_str() {
        let trimmed = s.trim();
        if let Ok(i) = trimmed.parse::<i32>() {
            return Some(i);
        }
        if let Some(f) = trimmed.parse::<f64>().ok().filter(|x| x.is_finite()) {
            return Some(f.round() as i32);
        }
    }
    None
}

fn json_opt_f64(val: Option<&serde_json::Value>) -> Option<f64> {
    let v = val?;
    if v.is_null() {
        return None;
    }
    if let Some(f) = v.as_f64().filter(|x| x.is_finite()) {
        return Some(f);
    }
    v.as_str()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|x| x.is_finite())
}

fn json_opt_nonempty_str(val: Option<&serde_json::Value>) -> Option<String> {
    val.and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn format_garmin_local_timestamp(val: Option<&serde_json::Value>) -> Option<String> {
    let v = val?;
    if v.is_null() {
        return None;
    }
    if let Some(num) = v.as_f64().filter(|x| x.is_finite() && *x > 0.0) {
        let ms = if num > 10_000_000_000.0 {
            num.round() as i64
        } else {
            (num * 1000.0).round() as i64
        };
        if let Some(dt) = DateTime::<Utc>::from_timestamp_millis(ms) {
            return Some(dt.format("%Y-%m-%dT%H:%M:%S").to_string());
        }
    }
    if let Some(s) = v.as_str() {
        let trimmed = s.trim();
        if trimmed.len() >= 19 {
            let normalized = trimmed[..19].replace(' ', "T");
            if NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S").is_ok() {
                return Some(normalized);
            }
        }
    }
    None
}

fn format_garmin_gmt_to_local_timestamp(val: Option<&serde_json::Value>) -> Option<String> {
    let v = val?;
    if v.is_null() {
        return None;
    }
    if let Some(num) = v.as_f64().filter(|x| x.is_finite() && *x > 0.0) {
        let ms = if num > 10_000_000_000.0 {
            num.round() as i64
        } else {
            (num * 1000.0).round() as i64
        };
        if let Some(dt) = DateTime::<Utc>::from_timestamp_millis(ms) {
            return Some(dt.with_timezone(&Local).format("%Y-%m-%dT%H:%M:%S").to_string());
        }
    }
    if let Some(s) = v.as_str() {
        let trimmed = s.trim();
        if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
            return Some(dt.with_timezone(&Local).format("%Y-%m-%dT%H:%M:%S").to_string());
        }
        if trimmed.len() >= 19 {
            let prefix = trimmed[..19].replace(' ', "T");
            if let Ok(ndt) = NaiveDateTime::parse_from_str(&prefix, "%Y-%m-%dT%H:%M:%S") {
                let dt = DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc);
                return Some(dt.with_timezone(&Local).format("%Y-%m-%dT%H:%M:%S").to_string());
            }
        }
    }
    None
}

pub fn parse_daily_summaries_json(bytes: &[u8], days_map: &mut HashMap<String, DayRecord>) {
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return;
    };
    let items: &[serde_json::Value] = if let Some(arr) = val.get("data").and_then(|v| v.as_array()) {
        arr.as_slice()
    } else if let Some(arr) = val.as_array() {
        arr.as_slice()
    } else {
        return;
    };

    for d in items {
        let Some(date) = json_opt_nonempty_str(d.get("calendarDate")) else {
            continue;
        };
        let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));
        if let Some(rhr) = json_opt_i32(d.pointer("/heartRate/restingValue")).filter(|&v| v > 0) {
            entry.resting_hr = Some(rhr);
        }
        if let Some(stress) = json_opt_i32(d.pointer("/stress/avgLevel")).filter(|&v| v >= 0) {
            entry.stress_avg = Some(stress);
        }
        if let Some(bb) = d.get("bodyBattery") {
            if let Some(max_v) = json_opt_i32(bb.get("maxValue")).or_else(|| json_opt_i32(bb.get("max"))) {
                entry.body_battery_high = Some(max_v);
            }
            if let Some(min_v) = json_opt_i32(bb.get("minValue")).or_else(|| json_opt_i32(bb.get("min"))) {
                entry.body_battery_low = Some(min_v);
            }
            if let Some(ch_v) = json_opt_i32(bb.get("chargedValue")).or_else(|| json_opt_i32(bb.get("charged"))) {
                entry.body_battery_charged = Some(ch_v);
            }
            if let Some(dr_v) = json_opt_i32(bb.get("drainedValue")).or_else(|| json_opt_i32(bb.get("drained"))) {
                entry.body_battery_drained = Some(dr_v);
            }
            if let Some(lat_v) = json_opt_i32(bb.get("latestValue")).or_else(|| json_opt_i32(bb.get("mostRecentValue"))) {
                entry.body_battery_latest = Some(lat_v);
            }
        }
        if let Some(steps_obj) = d.pointer("/movement/steps") {
            if let Some(steps) = json_opt_i32(steps_obj.get("count")).or_else(|| json_opt_i32(steps_obj.get("value"))) {
                entry.steps = Some(steps);
            }
            if let Some(goal) = json_opt_i32(steps_obj.get("goal")) {
                entry.step_goal = Some(goal);
            }
        }
    }
}

pub fn parse_sleep_summaries_json(bytes: &[u8], days_map: &mut HashMap<String, DayRecord>) {
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return;
    };
    let single_slice: [serde_json::Value; 1];
    let items: &[serde_json::Value] = if let Some(arr) = val.as_array() {
        arr.as_slice()
    } else if let Some(arr) = val.get("data").and_then(|v| v.as_array()) {
        arr.as_slice()
    } else if let Some(arr) = val.get("dailySleepSummaries").and_then(|v| v.as_array()) {
        arr.as_slice()
    } else if val.is_object() {
        single_slice = [val];
        &single_slice
    } else {
        return;
    };

    for outer in items {
        let dto = outer
            .get("dailySleepDTO")
            .filter(|v| v.is_object())
            .unwrap_or(outer);
        let Some(date) = json_opt_nonempty_str(dto.get("calendarDate"))
            .or_else(|| json_opt_nonempty_str(outer.get("calendarDate")))
        else {
            continue;
        };
        let Some(sleep_sec) = json_opt_i32(dto.get("sleepTimeSeconds"))
            .or_else(|| json_opt_i32(outer.get("sleepTimeSeconds")))
            .filter(|&s| s > 0)
        else {
            continue;
        };

        let score = json_opt_i32(dto.pointer("/sleepScores/overall/value"))
            .or_else(|| json_opt_i32(outer.pointer("/sleepScores/overall/value")))
            .or_else(|| json_opt_i32(dto.pointer("/sleepScores/overall")))
            .or_else(|| json_opt_i32(outer.pointer("/sleepScores/overall")))
            .or_else(|| json_opt_i32(dto.get("sleepScore")))
            .or_else(|| json_opt_i32(outer.get("sleepScore")))
            .or_else(|| json_opt_i32(dto.get("overallSleepScore")))
            .or_else(|| json_opt_i32(outer.get("overallSleepScore")));

        let deep = json_opt_i32(dto.get("deepSleepSeconds")).or_else(|| json_opt_i32(outer.get("deepSleepSeconds")));
        let light = json_opt_i32(dto.get("lightSleepSeconds")).or_else(|| json_opt_i32(outer.get("lightSleepSeconds")));
        let rem = json_opt_i32(dto.get("remSleepSeconds")).or_else(|| json_opt_i32(outer.get("remSleepSeconds")));
        let awake = json_opt_i32(dto.get("awakeSleepSeconds")).or_else(|| json_opt_i32(outer.get("awakeSleepSeconds")));

        let sleep_start = format_garmin_local_timestamp(dto.get("sleepStartTimestampLocal"))
            .or_else(|| format_garmin_local_timestamp(outer.get("sleepStartTimestampLocal")))
            .or_else(|| format_garmin_gmt_to_local_timestamp(dto.get("sleepStartTimestampGMT")))
            .or_else(|| format_garmin_gmt_to_local_timestamp(outer.get("sleepStartTimestampGMT")));
        let sleep_end = format_garmin_local_timestamp(dto.get("sleepEndTimestampLocal"))
            .or_else(|| format_garmin_local_timestamp(outer.get("sleepEndTimestampLocal")))
            .or_else(|| format_garmin_gmt_to_local_timestamp(dto.get("sleepEndTimestampGMT")))
            .or_else(|| format_garmin_gmt_to_local_timestamp(outer.get("sleepEndTimestampGMT")));

        let sleep_need_min = json_opt_i32(dto.pointer("/sleepNeed/actual"))
            .or_else(|| json_opt_i32(outer.pointer("/sleepNeed/actual")));
        let sleep_baseline_min = json_opt_i32(dto.pointer("/sleepNeed/baseline"))
            .or_else(|| json_opt_i32(outer.pointer("/sleepNeed/baseline")));
        let awake_count = json_opt_i32(dto.get("awakeCount"))
            .or_else(|| json_opt_i32(outer.get("awakeCount")));
        let avg_hr = json_opt_f64(dto.get("avgHeartRate"))
            .or_else(|| json_opt_f64(outer.get("avgHeartRate")));
        let sleep_stress = json_opt_f64(dto.get("avgSleepStress"))
            .or_else(|| json_opt_f64(outer.get("avgSleepStress")));
        let resp_avg = json_opt_f64(dto.get("averageRespirationValue"))
            .or_else(|| json_opt_f64(outer.get("averageRespirationValue")));
        let resp_low = json_opt_f64(dto.get("lowestRespirationValue"))
            .or_else(|| json_opt_f64(outer.get("lowestRespirationValue")));
        let resp_high = json_opt_f64(dto.get("highestRespirationValue"))
            .or_else(|| json_opt_f64(outer.get("highestRespirationValue")));
        let quality = json_opt_nonempty_str(dto.pointer("/sleepScores/overall/qualifierKey"))
            .or_else(|| json_opt_nonempty_str(outer.pointer("/sleepScores/overall/qualifierKey")));

        let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));
        // Guard against shorter naps or partial records overwriting the main night's sleep
        if let Some(existing_sec) = entry.sleep_seconds.filter(|&s| s > sleep_sec) {
            let _ = existing_sec;
            if entry.sleep_score.is_none() {
                entry.sleep_score = score;
            }
            continue;
        }

        entry.sleep_seconds = Some(sleep_sec);
        entry.sleep_duration_sec = Some(sleep_sec);
        if score.is_some() {
            entry.sleep_score = score;
        }
        if sleep_start.is_some() {
            entry.sleep_start = sleep_start;
        }
        if sleep_end.is_some() {
            entry.sleep_end = sleep_end;
        }
        if deep.is_some() {
            entry.sleep_deep_sec = deep;
        }
        if light.is_some() {
            entry.sleep_light_sec = light;
        }
        if rem.is_some() {
            entry.sleep_rem_sec = rem;
        }
        if awake.is_some() {
            entry.sleep_awake_sec = awake;
        }
        if sleep_need_min.is_some() {
            entry.sleep_need_minutes = sleep_need_min;
        }
        if sleep_baseline_min.is_some() {
            entry.sleep_baseline_minutes = sleep_baseline_min;
        }
        if awake_count.is_some() {
            entry.sleep_awake_count = awake_count;
        }
        if avg_hr.is_some() {
            entry.sleep_avg_hr = avg_hr;
        }
        if sleep_stress.is_some() {
            entry.sleep_stress = sleep_stress;
        }
        if resp_avg.is_some() {
            entry.sleep_respiration_avg = resp_avg;
        }
        if resp_low.is_some() {
            entry.sleep_respiration_low = resp_low;
        }
        if resp_high.is_some() {
            entry.sleep_respiration_high = resp_high;
        }
        if quality.is_some() {
            entry.sleep_quality = quality;
        }
    }
}

pub fn parse_hrv_range_json(bytes: &[u8], days_map: &mut HashMap<String, DayRecord>) {
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return;
    };
    let items: &[serde_json::Value] = if let Some(arr) = val.get("hrvSummaries").and_then(|v| v.as_array()) {
        arr.as_slice()
    } else if let Some(arr) = val.as_array() {
        arr.as_slice()
    } else {
        return;
    };

    for h in items {
        let Some(date) = json_opt_nonempty_str(h.get("calendarDate")) else {
            continue;
        };
        let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));
        if let Some(v) = json_opt_i32(h.get("lastNightAvg")) {
            entry.hrv_last_night = Some(v);
        }
        if let Some(v) = json_opt_i32(h.get("lastNight5MinHigh")) {
            entry.hrv_last_night_5min_high = Some(v);
        }
        if let Some(v) = json_opt_i32(h.get("weeklyAvg")) {
            entry.hrv_weekly_avg = Some(v);
        }
        if let Some(status) = json_opt_nonempty_str(h.get("status")) {
            entry.hrv_status = Some(status);
        }
        if let Some(baseline) = h.get("baseline") {
            if let Some(low) = json_opt_i32(baseline.get("balancedLow")) {
                entry.hrv_baseline_low = Some(low);
            }
            if let Some(high) = json_opt_i32(baseline.get("balancedUpper"))
                .or_else(|| json_opt_i32(baseline.get("balancedHigh")))
            {
                entry.hrv_baseline_high = Some(high);
            }
        }
    }
}

pub fn parse_hrv_night_json(date: &str, bytes: &[u8]) -> NightHrvDetail {
    let mut detail = NightHrvDetail {
        date: date.to_string(),
        ..Default::default()
    };
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return detail;
    };
    if let Some(summary) = val.get("hrvSummary") {
        detail.last_night_avg = json_opt_i32(summary.get("lastNightAvg"));
        detail.last_night_5min_high = json_opt_i32(summary.get("lastNight5MinHigh"));
        if let Some(baseline) = summary.get("baseline") {
            detail.baseline_low = json_opt_i32(baseline.get("balancedLow"));
            detail.baseline_high = json_opt_i32(baseline.get("balancedUpper"))
                .or_else(|| json_opt_i32(baseline.get("balancedHigh")));
        }
    }
    if let Some(readings) = val.get("hrvReadings").and_then(|v| v.as_array()) {
        let mut parsed: Vec<NightHrvReading> = Vec::new();
        for r in readings {
            let Some(value_ms) = json_opt_f64(r.get("hrvValue")).filter(|&v| v > 0.0) else {
                continue;
            };
            let Some(raw_ts) = json_opt_nonempty_str(r.get("readingTimeGMT")) else {
                continue;
            };
            let prefix: String = raw_ts.chars().take(19).collect();
            let dt_utc = if let Ok(dt) = DateTime::parse_from_rfc3339(&raw_ts) {
                Some(dt.with_timezone(&Utc))
            } else if let Ok(ndt) = NaiveDateTime::parse_from_str(&prefix, "%Y-%m-%dT%H:%M:%S") {
                Some(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc))
            } else {
                None
            };
            let Some(dt) = dt_utc else {
                continue;
            };
            parsed.push(NightHrvReading {
                timestamp_iso: dt.with_timezone(&Local).format("%Y-%m-%dT%H:%M:%S").to_string(),
                timestamp_ms: dt.timestamp_millis(),
                value_ms,
            });
        }
        parsed.sort_by_key(|r| r.timestamp_ms);
        detail.readings = parsed;
    }
    detail
}

pub fn parse_steps_json(bytes: &[u8], days_map: &mut HashMap<String, DayRecord>) {
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return;
    };
    let Some(items) = val.as_array() else {
        return;
    };
    for s in items {
        let Some(date) = json_opt_nonempty_str(s.get("calendarDate")) else {
            continue;
        };
        let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));
        if let Some(steps) = json_opt_i32(s.get("totalSteps")) {
            entry.steps = Some(steps);
        }
        if let Some(goal) = json_opt_i32(s.get("stepGoal")) {
            entry.step_goal = Some(goal);
        }
    }
}

pub fn parse_body_battery_days_json(bytes: &[u8], days_map: &mut HashMap<String, DayRecord>) {
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return;
    };
    let single_slice: [serde_json::Value; 1];
    let items: &[serde_json::Value] = if let Some(arr) = val.as_array() {
        arr.as_slice()
    } else if val.is_object() {
        single_slice = [val];
        &single_slice
    } else {
        return;
    };

    for item in items {
        let Some(date) = json_opt_nonempty_str(item.get("date"))
            .or_else(|| json_opt_nonempty_str(item.get("calendarDate")))
        else {
            continue;
        };
        let entry = days_map.entry(date.clone()).or_insert_with(|| empty_day(&date));

        if let Some(ch) = json_opt_i32(item.get("charged")).or_else(|| json_opt_i32(item.get("chargedValue"))) {
            entry.body_battery_charged = Some(ch);
        }
        if let Some(dr) = json_opt_i32(item.get("drained")).or_else(|| json_opt_i32(item.get("drainedValue"))) {
            entry.body_battery_drained = Some(dr);
        }

        let arr_opt = item
            .get("bodyBatteryValuesArray")
            .or_else(|| item.get("bodyBatteryValueArray"))
            .and_then(|v| v.as_array());
        if let Some(raw_points) = arr_opt {
            let mut points: Vec<BodyBatteryPoint> = Vec::new();
            for pair in raw_points {
                let Some(pair_arr) = pair.as_array() else {
                    continue;
                };
                if pair_arr.len() < 2 {
                    continue;
                }
                let Some(ts_raw) = json_opt_f64(pair_arr.first()) else {
                    continue;
                };
                let Some(lvl) = json_opt_i32(pair_arr.get(1)) else {
                    continue;
                };
                if !(0..=100).contains(&lvl) {
                    continue;
                }
                let ts_ms = if ts_raw > 10_000_000_000.0 {
                    ts_raw.round() as i64
                } else {
                    (ts_raw * 1000.0).round() as i64
                };
                points.push(BodyBatteryPoint {
                    timestamp_ms: ts_ms,
                    level: lvl,
                });
            }
            points.sort_by_key(|p| p.timestamp_ms);
            points.dedup_by_key(|p| p.timestamp_ms);
            if !points.is_empty() {
                if let Some(last) = points.last() {
                    entry.body_battery_latest = Some(last.level);
                }
                if let Some(pts_max) = points.iter().map(|p| p.level).max() {
                    entry.body_battery_high = Some(
                        entry
                            .body_battery_high
                            .map_or(pts_max, |cur| cur.max(pts_max)),
                    );
                }
                if let Some(pts_min) = points.iter().map(|p| p.level).min() {
                    entry.body_battery_low = Some(
                        entry
                            .body_battery_low
                            .map_or(pts_min, |cur| cur.min(pts_min)),
                    );
                }
                entry.body_battery_series = Some(points);
            }
        }
    }
}

fn empty_day(date: &str) -> DayRecord {
    DayRecord {
        date: date.to_string(),
        ..Default::default()
    }
}

fn normalize_optional_text(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActivityMetadataUpdate {
    pub garmin_id: i64,
    pub name: Option<String>,
    pub garmin_notes: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityDTO {
    activity_id: i64,
    activity_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
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
    #[serde(default, rename = "isPR")]
    is_pr: Option<bool>,
    #[serde(default)]
    start_latitude: Option<f64>,
    #[serde(default)]
    start_longitude: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityDetailDTO {
    activity_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "metadataDTO")]
    metadata_dto: Option<serde_json::Value>,
    #[serde(default, rename = "summaryDTO")]
    summary_dto: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityTypeDTO {
    type_key: Option<String>,
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

fn is_external_tempe_device_info(msg: &fitparser::FitDataRecord) -> bool {
    let mut is_local = false;
    let mut has_env_type = false;
    let mut has_tempe_product = false;

    for field in msg.fields() {
        let name = field.name();
        let val = field.value();
        match name {
            "source_type" => {
                if let fitparser::Value::String(s) = val {
                    if s.eq_ignore_ascii_case("local") {
                        is_local = true;
                    }
                } else if value_as_f64(val) == Some(5.0) {
                    is_local = true;
                }
            }
            "device_type" | "antplus_device_type" | "ble_device_type" => {
                if let fitparser::Value::String(s) = val {
                    let lower = s.to_lowercase();
                    if lower.contains("env_sensor")
                        || lower.contains("environment_sensor")
                        || lower == "tempe"
                    {
                        has_env_type = true;
                    }
                } else if value_as_f64(val) == Some(25.0) {
                    has_env_type = true;
                }
            }
            "garmin_product" | "product" => {
                if let fitparser::Value::String(s) = val {
                    if s.eq_ignore_ascii_case("tempe") {
                        has_tempe_product = true;
                    }
                } else if value_as_f64(val) == Some(1570.0) {
                    has_tempe_product = true;
                }
            }
            _ => {}
        }
    }

    (has_env_type && !is_local) || has_tempe_product
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
    let mut temp_source = default_temp_source();

    for msg in records {
        if msg.kind() == fitparser::profile::MesgNum::DeviceInfo {
            if is_external_tempe_device_info(&msg) {
                temp_source = "garmin_tempe_fit".to_string();
            }
            continue;
        }
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
        let mut power_w: Option<f64> = None;
        let mut gct_ms: Option<f64> = None;
        let mut stance_time_pct: Option<f64> = None;
        let mut vertical_oscillation_mm: Option<f64> = None;
        let mut vertical_ratio_pct: Option<f64> = None;
        let mut step_length_mm: Option<f64> = None;
        let mut stance_balance_left_pct: Option<f64> = None;
        let mut temperature_c: Option<f64> = None;
        let mut respiration_brpm: Option<f64> = None;
        let mut latitude_deg: Option<f64> = None;
        let mut longitude_deg: Option<f64> = None;
        let mut impact_load: Option<f64> = None;

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
                "power" => power_w = value_as_f64(field.value()),
                "stance_time" => gct_ms = value_as_f64(field.value()),
                "stance_time_percent" => stance_time_pct = value_as_f64(field.value()),
                "vertical_oscillation" => vertical_oscillation_mm = value_as_f64(field.value()),
                "vertical_ratio" => vertical_ratio_pct = value_as_f64(field.value()),
                "step_length" => step_length_mm = value_as_f64(field.value()),
                "stance_time_balance" => stance_balance_left_pct = value_as_f64(field.value()),
                "temperature" => temperature_c = value_as_f64(field.value()),
                "enhanced_respiration_rate" | "respiration_rate" => {
                    if respiration_brpm.is_none() || field.name() == "enhanced_respiration_rate" {
                        respiration_brpm = value_as_f64(field.value());
                    }
                }
                "position_lat" => {
                    latitude_deg = value_as_f64(field.value()).map(|v| v * (180.0 / 2_147_483_648.0));
                }
                "position_long" => {
                    longitude_deg = value_as_f64(field.value()).map(|v| v * (180.0 / 2_147_483_648.0));
                }
                "impact_load" => impact_load = value_as_f64(field.value()),
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
            power_w,
            gct_ms,
            stance_time_pct,
            vertical_oscillation_mm,
            vertical_ratio_pct,
            step_length_mm,
            stance_balance_left_pct,
            temperature_c,
            respiration_brpm,
            latitude_deg,
            longitude_deg,
            impact_load,
        });
    }

    let mut analysis = analyze_fit_points(garmin_id, points, config);
    analysis.compromised_run_audit = CompromisedRunAudit::compute(&laps);
    analysis.hyrox_dissection = dissect_hyrox_laps(&laps, false);
    analysis.temp_source = temp_source.clone();
    analysis.forensic_telemetry = Some(crate::forensic_extractor::extract_forensic_telemetry_with_source(
        &analysis.full_points,
        &laps,
        &temp_source,
    ));
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
        if force_hyrox {
            return Some(HyroxDissection {
                is_hyrox_sim: true,
                run_splits: Vec::new(),
                station_splits: Vec::new(),
                roxzone_transitions: Vec::new(),
                clean_run_pace_sec_per_km: None,
                clean_aerobic_efficiency: None,
                compromised_pace_decay_sec: None,
                station_hr_recovery_sec: None,
            });
        }
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

    let forensic_telemetry = Some(crate::forensic_extractor::extract_forensic_telemetry(
        &full_points,
        &[],
    ));

    FitTrackAnalysis {
        garmin_id,
        chart_points,
        full_points,
        splits,
        laps: Vec::new(),
        compromised_run_audit: None,
        hyrox_dissection: None,
        forensic_telemetry,
        temp_source: default_temp_source(),
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

/// Extracts a concise text summary from a user-uploaded `.fit` file (e.g., Garmin Stress/Lactate test)
/// so it can be attached to onboarding intake Coach prompts.
pub fn summarize_fit_file_for_intake(path: &std::path::Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read .fit file: {e}"))?;
    let records = fitparser::from_bytes(&bytes).map_err(|e| format!("FIT parse error: {e}"))?;

    let mut sport: Option<String> = None;
    let mut sub_sport: Option<String> = None;
    let mut total_dist_m: Option<f64> = None;
    let mut total_elapsed_s: Option<f64> = None;
    let mut avg_hr: Option<f64> = None;
    let mut max_hr: Option<f64> = None;
    let mut min_hr: Option<f64> = None;
    let mut threshold_hr: Option<f64> = None;
    let mut lactate_vals: Vec<String> = Vec::new();
    let mut lap_lines: Vec<String> = Vec::new();
    let mut record_hr_min: Option<f64> = None;
    let mut record_hr_max: Option<f64> = None;
    let mut record_count = 0usize;

    for msg in records {
        match msg.kind() {
            fitparser::profile::MesgNum::Session => {
                for field in msg.fields() {
                    match field.name() {
                        "sport" => sport = Some(format!("{:?}", field.value())),
                        "sub_sport" => sub_sport = Some(format!("{:?}", field.value())),
                        "total_distance" => total_dist_m = value_as_f64(field.value()),
                        "total_elapsed_time" | "total_timer_time" => {
                            if total_elapsed_s.is_none() || field.name() == "total_timer_time" {
                                total_elapsed_s = value_as_f64(field.value());
                            }
                        }
                        "avg_heart_rate" => avg_hr = value_as_f64(field.value()),
                        "max_heart_rate" => max_hr = value_as_f64(field.value()),
                        "min_heart_rate" => min_hr = value_as_f64(field.value()),
                        "threshold_heart_rate" | "functional_threshold_heart_rate" => {
                            threshold_hr = value_as_f64(field.value());
                        }
                        other if other.contains("lactate") => {
                            lactate_vals.push(format!("{other}: {:?}", field.value()));
                        }
                        _ => {}
                    }
                }
            }
            fitparser::profile::MesgNum::Lap => {
                let mut d = 0.0_f64;
                let mut t = 0.0_f64;
                let mut l_avg_hr: Option<f64> = None;
                let mut l_max_hr: Option<f64> = None;
                for field in msg.fields() {
                    match field.name() {
                        "total_distance" => d = value_as_f64(field.value()).unwrap_or(0.0),
                        "total_timer_time" | "total_elapsed_time" => {
                            if t <= 0.0 || field.name() == "total_timer_time" {
                                t = value_as_f64(field.value()).unwrap_or(0.0);
                            }
                        }
                        "avg_heart_rate" => l_avg_hr = value_as_f64(field.value()),
                        "max_heart_rate" => l_max_hr = value_as_f64(field.value()),
                        other if other.contains("lactate") || other.contains("threshold") => {
                            lactate_vals.push(format!("lap#{} {other}: {:?}", lap_lines.len() + 1, field.value()));
                        }
                        _ => {}
                    }
                }
                if lap_lines.len() < 16 && (t > 0.0 || d > 0.0 || l_avg_hr.is_some()) {
                    lap_lines.push(format!(
                        "  - Lap {}: {:.0}s, {:.2} km, avg HR {}, max HR {}",
                        lap_lines.len() + 1,
                        t,
                        d / 1000.0,
                        l_avg_hr.map_or("–".to_string(), |v| format!("{:.0}", v)),
                        l_max_hr.map_or("–".to_string(), |v| format!("{:.0}", v)),
                    ));
                }
            }
            fitparser::profile::MesgNum::Record => {
                record_count += 1;
                for field in msg.fields() {
                    if field.name() == "heart_rate" {
                        if let Some(hr) = value_as_f64(field.value()).filter(|&h| h > 30.0 && h < 235.0) {
                            record_hr_min = Some(record_hr_min.map_or(hr, |m| m.min(hr)));
                            record_hr_max = Some(record_hr_max.map_or(hr, |m| m.max(hr)));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("uploaded.fit");
    let mut out = vec![
        format!("File: {file_name} (records: {record_count}, laps: {})", lap_lines.len()),
        format!(
            "Sport: {} / {}",
            sport.as_deref().unwrap_or("unknown"),
            sub_sport.as_deref().unwrap_or("generic")
        ),
    ];
    if let Some(d) = total_dist_m {
        out.push(format!("Distance: {:.2} km", d / 1000.0));
    }
    if let Some(t) = total_elapsed_s {
        out.push(format!("Duration: {:.0} s ({:.1} min)", t, t / 60.0));
    }
    let eff_max = max_hr.or(record_hr_max);
    let eff_min = min_hr.or(record_hr_min);
    out.push(format!(
        "HR: avg {} bpm, max {} bpm, min {} bpm",
        avg_hr.map_or("–".to_string(), |v| format!("{v:.0}")),
        eff_max.map_or("–".to_string(), |v| format!("{v:.0}")),
        eff_min.map_or("–".to_string(), |v| format!("{v:.0}")),
    ));
    if let Some(thr) = threshold_hr {
        out.push(format!("FIT threshold_heart_rate: {thr:.0} bpm"));
    }
    if !lactate_vals.is_empty() {
        out.push(format!("Lactate/threshold fields: {}", lactate_vals.join("; ")));
    }
    if !lap_lines.is_empty() {
        out.push("Lap breakdown:".to_string());
        out.extend(lap_lines);
    }
    Ok(out.join("\n"))
}
