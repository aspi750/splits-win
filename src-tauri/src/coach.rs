use crate::models::*;
use crate::readiness::{ReadinessEngine, ReadinessInput};
use crate::storage::{CredentialStore, Database, SharedStore};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Utc};
use std::collections::BTreeMap;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

pub const GEMINI_MODEL_ID: &str = "gemini-4-hydrogen";

pub struct CoachPacketBuilder;

impl CoachPacketBuilder {
    /// Builds a clean, token-optimized metric Markdown bundle (`CONTRACT.md §4.2`).
    pub fn build_markdown(
        config: &AthleteConfig,
        activities: &[ActivityRecord],
        days: &[DayRecord],
        gear_inventory: Option<&GearInventoryFile>,
        readiness_state: Option<&ReadinessState>,
        draft_prompt: &str,
        now: DateTime<Utc>,
    ) -> String {
        let mut sections = Vec::new();

        let rhr_samples: Vec<(DateTime<Utc>, Option<i32>)> = days
            .iter()
            .filter_map(|d| {
                NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")
                    .ok()
                    .and_then(|nd| nd.and_hms_opt(12, 0, 0))
                    .map(|ndt| (DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc), d.resting_hr))
            })
            .collect();
        let effective_rhr = config.effective_resting_hr_baseline(&rhr_samples, now);

        // 1. Athlete Profile & Active Guardrails
        let mut profile_sec = format!(
            "## Athlete Profile & Active Guardrails\n{}",
            config.prompt_summary(effective_rhr)
        );
        if let Some(r) = readiness_state {
            let status_label = if r.gatekeeper.run_permitted { "Run Permitted" } else { "Rest / Restricted" };
            profile_sec.push_str(&format!(
                "\n- Live Readiness: {status_label} ({}, HR ceiling {} bpm)",
                r.prescription_summary.decision, r.prescription_summary.hr_ceiling_bpm
            ));
            if !r.gatekeeper.hard_blocks.is_empty() {
                profile_sec.push_str(&format!(
                    "\n- Active Hard Blocks: {}",
                    r.gatekeeper.hard_blocks.join("; ")
                ));
            }
        }
        sections.push(profile_sec);

        // 2. 7-Day & 28-Day Load Summary
        let seven_ago = now - Duration::days(7);
        let twenty_eight_ago = now - Duration::days(28);
        let acts_7d: Vec<&ActivityRecord> = activities
            .iter()
            .filter(|a| parse_iso(&a.start_iso).map_or(false, |dt| dt >= seven_ago && dt <= now))
            .collect();
        let acts_28d: Vec<&ActivityRecord> = activities
            .iter()
            .filter(|a| parse_iso(&a.start_iso).map_or(false, |dt| dt >= twenty_eight_ago && dt <= now))
            .collect();

        let dist_7d: f64 = acts_7d.iter().map(|a| a.distance_km()).sum();
        let dur_7d_min: f64 = acts_7d.iter().map(|a| a.moving_duration_sec / 60.0).sum();
        let dist_28d: f64 = acts_28d.iter().map(|a| a.distance_km()).sum();
        let dur_28d_min: f64 = acts_28d.iter().map(|a| a.moving_duration_sec / 60.0).sum();
        let chronic_weekly = dist_28d / 4.0;
        let acwr_str = if chronic_weekly > 0.1 {
            format!("{:.2}", dist_7d / chronic_weekly)
        } else {
            "N/A".to_string()
        };

        let mut sorted_days = days.to_vec();
        sorted_days.sort_by(|a, b| b.date.cmp(&a.date));
        let recent_7: Vec<&DayRecord> = sorted_days.iter().take(7).collect();
        let rhr_vals: Vec<i32> = recent_7.iter().filter_map(|d| d.resting_hr).collect();
        let rhr_avg = if rhr_vals.is_empty() {
            None
        } else {
            Some(rhr_vals.iter().sum::<i32>() as f64 / rhr_vals.len() as f64)
        };
        let rhr_line = match (sorted_days.first().and_then(|d| d.resting_hr), effective_rhr) {
            (Some(latest), Some(base)) => {
                let delta = latest - base;
                let sign = if delta >= 0 { "+" } else { "" };
                format!("latest {latest} bpm ({sign}{delta} vs {base} bpm baseline)")
            }
            (Some(latest), None) => format!("latest {latest} bpm"),
            (None, Some(base)) => format!("baseline {base} bpm (no reading today)"),
            (None, None) => "N/A".to_string(),
        };
        let rhr_full = if let Some(avg) = rhr_avg {
            format!("Resting HR: {rhr_line} · 7d avg {avg:.1} bpm")
        } else {
            format!("Resting HR: {rhr_line}")
        };

        let sleep_hrs: Vec<f64> = recent_7
            .iter()
            .filter_map(|d| d.sleep_seconds.map(|s| s as f64 / 3600.0))
            .collect();
        let sleep_text = if sleep_hrs.is_empty() {
            "N/A".to_string()
        } else {
            let avg = sleep_hrs.iter().sum::<f64>() / sleep_hrs.len() as f64;
            format!("{avg:.1} h/night ({} nights)", sleep_hrs.len())
        };

        let hrv_vals: Vec<i32> = recent_7.iter().filter_map(|d| d.hrv_last_night).collect();
        let hrv_text = if hrv_vals.is_empty() {
            "N/A".to_string()
        } else {
            let avg = hrv_vals.iter().sum::<i32>() as f64 / hrv_vals.len() as f64;
            format!("{avg:.0} ms avg ({} nights)", hrv_vals.len())
        };

        sections.push(format!(
            "## 7-Day & 28-Day Load Summary\n- 7-Day Volume: {dist_7d:.1} km · {} ({} activities)\n- 28-Day Volume: {dist_28d:.1} km · {} ({} activities)\n- Acute:Chronic Workload Ratio (ACWR, distance): {acwr_str}\n- {rhr_full}\n- 7-Day Sleep: {sleep_text}\n- 7-Day HRV: {hrv_text}",
            format_minutes(dur_7d_min),
            acts_7d.len(),
            format_minutes(dur_28d_min),
            acts_28d.len()
        ));

        // 3. Recent Activities Table (last 10)
        let mut recent_acts = activities.to_vec();
        recent_acts.sort_by(|a, b| b.start_iso.cmp(&a.start_iso));
        recent_acts.truncate(10);
        if recent_acts.is_empty() {
            sections.push("## Recent Activities\nNo synced activities available.".to_string());
        } else {
            let mut lines = vec![
                format!("## Recent Activities (Last {})", recent_acts.len()),
                "| Date | Sport / Title | Distance | Duration | Avg Pace | Avg/Max HR | Decoupling | Shoe |".to_string(),
                "| --- | --- | --- | --- | --- | --- | --- | --- |".to_string(),
            ];
            for act in recent_acts {
                let date_str = act.start_iso.chars().take(10).collect::<String>();
                let title = if act.name.trim().is_empty() {
                    act.type_key.clone()
                } else {
                    format!("{}: {}", act.type_key, act.name.trim())
                };
                let km = act.distance_km();
                let dist_str = if km > 0.05 { format!("{km:.2} km") } else { "–".to_string() };
                let dur_sec = if act.moving_duration_sec > 0.0 { act.moving_duration_sec } else { act.duration_sec };
                let dur_str = format_duration(dur_sec);
                let pace_str = if km > 0.1 && act.moving_duration_sec > 0.0 && act.type_key.to_lowercase().contains("run") {
                    format!("{}/km", AthleteConfig::format_pace((act.moving_duration_sec / km).round() as i32))
                } else {
                    "–".to_string()
                };
                let hr_str = match (act.average_hr, act.max_hr) {
                    (Some(a), Some(m)) => format!("{}/{} bpm", a.round() as i32, m.round() as i32),
                    (Some(a), None) => format!("{} bpm", a.round() as i32),
                    _ => "–".to_string(),
                };
                let shoe_str = act.shoe_name.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("–");
                lines.push(format!("| {date_str} | {title} | {dist_str} | {dur_str} | {pace_str} | {hr_str} | – | {shoe_str} |"));
            }
            sections.push(lines.join("\n"));
        }

        // 4. Active Shoe Rotation
        if let Some(inv) = gear_inventory {
            let active: Vec<&GearItem> = inv
                .items
                .values()
                .filter(|item| item.footwear_status() != "blocked")
                .collect();
            if !active.is_empty() {
                let mut lines = vec!["## Active Shoe Rotation".to_string()];
                for item in active {
                    let km = item.garmin.as_ref().and_then(|g| g.distance_used_m).unwrap_or(0.0) / 1000.0;
                    lines.push(format!("- {}: {km:.1} km ({})", item.display_name(), item.footwear_status()));
                }
                sections.push(lines.join("\n"));
            }
        }

        // 5. Coaching Request
        let trimmed = draft_prompt.trim();
        if trimmed.is_empty() {
            sections.push("## Coaching Request\nPlease review my recent load, recovery trends, and activity telemetry above against my active guardrails and objective, and recommend my next session and weekly adjustments.".to_string());
        } else {
            sections.push(format!("## Coaching Request\n{trimmed}"));
        }

        sections.join("\n\n")
    }
}

pub fn resolve_coach_engine_mode() -> CoachEngineMode {
    if CredentialStore::read_secret(APP_SERVICE, GEMINI_ACCOUNT).is_some() {
        CoachEngineMode::GeminiApi
    } else {
        CoachEngineMode::ClipboardPacket
    }
}

pub fn replacing_section(markdown: &str, section_markdown: &str) -> String {
    let trimmed = section_markdown.trim();
    let Some(heading_line) = trimmed.lines().find(|l| l.starts_with("## ")) else {
        return if markdown.is_empty() {
            format!("{trimmed}\n")
        } else {
            format!("{}\n\n{trimmed}\n", markdown.trim_end())
        };
    };

    let lines: Vec<&str> = markdown.lines().collect();
    if let Some(start_idx) = lines.iter().position(|l| l.trim() == heading_line.trim()) {
        let mut end_idx = lines.len();
        for (i, line) in lines.iter().enumerate().skip(start_idx + 1) {
            if line.starts_with("## ") {
                end_idx = i;
                break;
            }
        }
        let mut out = Vec::new();
        out.extend_from_slice(&lines[..start_idx]);
        out.push(trimmed);
        out.extend_from_slice(&lines[end_idx..]);
        format!("{}\n", out.join("\n"))
    } else if markdown.is_empty() {
        format!("{trimmed}\n")
    } else {
        format!("{}\n\n{trimmed}\n", markdown.trim_end())
    }
}

pub fn gemini_tool_declarations() -> Value {
    json!([
        {
            "name": "propose_coach_update",
            "description": "Propose a reversible Markdown section update to COACH_PROFILE.md or TRAINING_PLAN.md.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "target_file": { "type": "STRING" },
                    "reason": { "type": "STRING" },
                    "updated_section_markdown": { "type": "STRING" },
                    "expires_on": { "type": "STRING" }
                },
                "required": ["target_file", "reason", "updated_section_markdown"]
            }
        },
        {
            "name": "query_run_slice",
            "description": "Return a compact 1 Hz telemetry slice for a given Garmin activity_id.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "activity_id": { "type": "INTEGER" },
                    "start_km": { "type": "NUMBER" },
                    "end_km": { "type": "NUMBER" }
                },
                "required": ["activity_id", "start_km", "end_km"]
            }
        },
        {
            "name": "get_recent_activities",
            "description": "Query synced activity store ordered newest-first.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "limit": { "type": "INTEGER" },
                    "sport": { "type": "STRING" },
                    "days_back": { "type": "INTEGER" }
                }
            }
        },
        {
            "name": "get_activity_detail",
            "description": "Return full analytical detail for a synced activity.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "activity_id": { "type": "STRING" },
                    "fetch_if_missing": { "type": "BOOLEAN" }
                },
                "required": ["activity_id"]
            }
        },
        {
            "name": "get_wellness_history",
            "description": "Return up to 60 days of daily recovery & wellness telemetry.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "days": { "type": "INTEGER" }
                }
            }
        },
        {
            "name": "get_training_load_and_records",
            "description": "Return rolling 7d/14d/28d volume, ACWR, personal bests, and active footwear.",
            "parameters": { "type": "OBJECT", "properties": {} }
        },
        {
            "name": "get_readiness_and_plan",
            "description": "Return readiness_state.json, plan.json, today_adapted_target.json, and coaching logs.",
            "parameters": { "type": "OBJECT", "properties": {} }
        },
        {
            "name": "get_lifetime_and_yearly_history",
            "description": "Return a two-tier training history overview: Tier 1 (lifetime totals, annual breakdown, peak 7d/28d) and Tier 2 (trailing 365-day block audit, consistency, run-to-hybrid ratio).",
            "parameters": { "type": "OBJECT", "properties": {} }
        },
        {
            "name": "fetch_1hz_window",
            "description": "Fetch raw 1 Hz second-by-second FIT telemetry rows (HR, pace, power, cadence, GCT, vertical oscillation, stride length, left stance balance %, temperature) between start_second and end_second.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "activity_id": { "type": "STRING" },
                    "start_second": { "type": "NUMBER" },
                    "end_second": { "type": "NUMBER" },
                    "step_seconds": { "type": "INTEGER" }
                },
                "required": ["activity_id", "start_second", "end_second"]
            }
        },
        {
            "name": "append_training_ledger",
            "description": "Append a structured markdown entry to ASPI_Training_Log.txt.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "entry_markdown": { "type": "STRING" }
                },
                "required": ["entry_markdown"]
            }
        },
        {
            "name": "update_post_run_context",
            "description": "Update an activity's PostRunContext with confirmed intra-run fluid intake (ml) and/or external Garmin Tempe usage, recomputing provisional vs final Sweat Matrix rates.",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "activity_id": { "type": "STRING" },
                    "fluid_consumed_ml": { "type": "NUMBER" },
                    "tempe_sensor_used": { "type": "BOOLEAN" }
                },
                "required": ["activity_id"]
            }
        }
    ])
}

pub fn apply_coach_proposal(store: &SharedStore, proposal: &CoachUpdateProposal) -> Result<(), String> {
    if proposal.target_file != "COACH_PROFILE.md" && proposal.target_file != "TRAINING_PLAN.md" {
        return Err(format!("Unsupported proposal target_file: {}", proposal.target_file));
    }
    let current = store.read_text(&proposal.target_file).map_err(|e| e.to_string())?.unwrap_or_default();
    let mut updated = replacing_section(&current, &proposal.updated_section_markdown);
    let today = Local::now().format("%Y-%m-%d").to_string();
    let history_entry = format!("- `{today}`: {} [target: {}]", proposal.reason, proposal.target_file);
    updated = append_history_entry(&history_entry, &updated);
    store
        .write_text_atomic(&proposal.target_file, &updated)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn append_history_entry(entry: &str, markdown: &str) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    if let Some(idx) = lines.iter().position(|l| l.trim() == "## Change History") {
        let mut next: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        next.insert(idx + 1, entry.to_string());
        format!("{}\n", next.join("\n"))
    } else {
        format!("{}\n\n## Change History\n{entry}\n", markdown.trim_end())
    }
}

pub fn slice_fit_track_by_km(track: &FitTrackAnalysis, start_km: f64, end_km: f64) -> Value {
    let start_m = start_km.max(0.0) * 1000.0;
    let end_m = end_km.max(start_km) * 1000.0;
    let pts: Vec<&FitTrackPoint> = track
        .full_points
        .iter()
        .filter(|p| p.distance_m >= start_m && p.distance_m <= end_m)
        .collect();
    json!({
        "garmin_id": track.garmin_id,
        "start_km": start_km,
        "end_km": end_km,
        "point_count": pts.len(),
        "points": pts.into_iter().take(180).collect::<Vec<_>>()
    })
}

fn tool_status_label(name: &str) -> &'static str {
    match name {
        "get_activity_detail" => "Auditing .fit track & decoupling…",
        "get_recent_activities" => "Reading recent activities…",
        "get_wellness_history" => "Analyzing sleep, HRV & wellness history…",
        "get_training_load_and_records" => "Computing training load & records…",
        "get_readiness_and_plan" => "Checking readiness gatekeeper & plan…",
        "get_lifetime_and_yearly_history" => "Auditing lifetime & 365-day training history…",
        "propose_coach_update" => "Drafting profile/plan update…",
        "query_run_slice" => "Inspecting run kilometer slice…",
        "fetch_1hz_window" => "Fetching 1Hz second-by-second window…",
        "append_training_ledger" => "Appending entry to ASPI_Training_Log.txt…",
        "update_post_run_context" => "Updating post-run hydration & Tempe status…",
        _ => "Running coach tool…",
    }
}

pub async fn run_gemini_coach_turn(
    app: Option<&AppHandle>,
    api_key: &str,
    system_instruction: &str,
    user_prompt: &str,
    store: &SharedStore,
    db: Option<&Database>,
    activities: &[ActivityRecord],
    days: &[DayRecord],
) -> Result<(String, Vec<CoachUpdateProposal>), String> {
    let client = Client::new();
    let endpoint = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{GEMINI_MODEL_ID}:streamGenerateContent?alt=sse&key={api_key}"
    );

    let mut contents = vec![json!({
        "role": "user",
        "parts": [{ "text": user_prompt }]
    })];
    let mut proposals = Vec::new();
    let mut final_text = String::new();

    for _round in 0..8 {
        let body = json!({
            "systemInstruction": { "parts": [{ "text": system_instruction }] },
            "contents": contents,
            "tools": [{ "functionDeclarations": gemini_tool_declarations() }],
            "generationConfig": {
                "temperature": 0.4,
                "maxOutputTokens": 8192,
                "thinkingConfig": { "thinkingLevel": "MEDIUM" }
            }
        });

        let resp = client
            .post(&endpoint)
            .header("x-goog-api-key", api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            let code = resp.status();
            if code.as_u16() == 429 {
                return Err("Gemini rate limit reached (HTTP 429). Please wait a moment before trying again.".to_string());
            }
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Gemini API error ({code}): {text}"));
        }

        let mut model_parts: Vec<Value> = Vec::new();
        let mut function_calls: Vec<(String, Value)> = Vec::new();
        let mut finish_reason = "STOP".to_string();
        let mut usage_entry: Option<CoachUsageEntry> = None;

        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| e.to_string())?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim_end_matches('\r').to_string();
                buffer.drain(..=pos);

                if !line.starts_with("data: ") {
                    continue;
                }
                let json_str = line["data: ".len()..].trim();
                if json_str.is_empty() || json_str == "[DONE]" {
                    continue;
                }
                let Ok(obj) = serde_json::from_str::<Value>(json_str) else {
                    continue;
                };

                if let Some(reason) = obj["candidates"][0]["finishReason"].as_str() {
                    finish_reason = reason.to_string();
                }

                if let Some(parts) = obj["candidates"][0]["content"]["parts"].as_array() {
                    for part in parts {
                        let is_thought = part["thought"].as_bool() == Some(true);
                        if let Some(fc) = part.get("functionCall") {
                            let name = fc["name"].as_str().unwrap_or_default().to_string();
                            let args = fc["args"].clone();
                            let mut fc_part = json!({ "functionCall": { "name": name, "args": args } });
                            if let Some(sig) = part.get("thoughtSignature").or_else(|| part.get("thought_signature")) {
                                fc_part["thoughtSignature"] = sig.clone();
                            }
                            model_parts.push(fc_part);
                            function_calls.push((name, args));
                        } else {
                            if is_thought || part.get("text").is_none() {
                                let mut normalized = part.clone();
                                if normalized.get("thoughtSignature").is_none() {
                                    if let Some(sig) = normalized.get("thought_signature").cloned() {
                                        if let Some(map) = normalized.as_object_mut() {
                                            map.insert("thoughtSignature".to_string(), sig);
                                            map.remove("thought_signature");
                                        }
                                    }
                                }
                                model_parts.push(normalized);
                            }
                            if !is_thought {
                                if let Some(delta) = part["text"].as_str() {
                                    if !delta.is_empty() {
                                        final_text.push_str(delta);
                                        if let Some(app_handle) = app {
                                            let _ = app_handle.emit("coach:delta", json!({ "delta": delta }));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(usage) = obj.get("usageMetadata") {
                    usage_entry = Some(CoachUsageEntry {
                        timestamp: Utc::now().to_rfc3339(),
                        profile: "CHAT".to_string(),
                        model: GEMINI_MODEL_ID.to_string(),
                        prompt_tokens: usage["promptTokenCount"].as_i64().unwrap_or(0) as i32,
                        candidates_tokens: usage["candidatesTokenCount"].as_i64().unwrap_or(0) as i32,
                        thoughts_tokens: usage["thoughtsTokenCount"].as_i64().unwrap_or(0) as i32,
                        total_tokens: usage["totalTokenCount"].as_i64().unwrap_or(0) as i32,
                        finish_reason: finish_reason.clone(),
                    });
                }
            }
        }

        if let Some(entry) = usage_entry {
            let _ = store.append_usage_log(&entry);
        }

        if function_calls.is_empty() {
            break;
        }

        let mut response_parts = Vec::new();
        for (name, args) in function_calls {
            let label = tool_status_label(&name);
            if let Some(app_handle) = app {
                let _ = app_handle.emit(
                    "coach:tool_status",
                    json!({ "tool": name, "label": label, "active": true }),
                );
            }
            let tool_resp = execute_tool(&name, &args, store, db, activities, days, &mut proposals);
            if let Some(app_handle) = app {
                let _ = app_handle.emit(
                    "coach:tool_status",
                    json!({ "tool": name, "label": label, "active": false }),
                );
            }
            response_parts.push(json!({
                "functionResponse": {
                    "name": name,
                    "response": tool_resp
                }
            }));
        }

        contents.push(json!({ "role": "model", "parts": model_parts }));
        contents.push(json!({ "role": "user", "parts": response_parts }));
    }

    Ok((final_text, proposals))
}

fn execute_tool(
    name: &str,
    args: &Value,
    store: &SharedStore,
    db: Option<&Database>,
    activities: &[ActivityRecord],
    days: &[DayRecord],
    proposals: &mut Vec<CoachUpdateProposal>,
) -> Value {
    match name {
        "propose_coach_update" => {
            let target = args["target_file"].as_str().unwrap_or("COACH_PROFILE.md").to_string();
            let reason = args["reason"].as_str().unwrap_or("Coach update").to_string();
            let section = args["updated_section_markdown"].as_str().unwrap_or("").to_string();
            let expires = args["expires_on"].as_str().map(|s| s.to_string());
            let id = uuid::Uuid::new_v4().to_string();
            proposals.push(CoachUpdateProposal {
                id: id.clone(),
                target_file: target.clone(),
                reason,
                updated_section_markdown: section.clone(),
                expires_on: expires,
                diff_preview: section,
                status: "pending_user_approval".to_string(),
            });
            json!({ "status": "pending_user_approval", "proposal_id": id, "target_file": target })
        }
        "query_run_slice" => {
            let activity_id = args["activity_id"].as_i64().unwrap_or(0);
            let start_km = args["start_km"].as_f64().unwrap_or(0.0);
            let end_km = args["end_km"].as_f64().unwrap_or(1.0);
            if let Some(Some(track)) = db.and_then(|d| d.get_fit_track(activity_id).ok()) {
                slice_fit_track_by_km(&track, start_km, end_km)
            } else {
                json!({ "error": format!("No cached .fit track found for activity_id {activity_id}") })
            }
        }
        "get_activity_detail" => {
            let raw_id = args["activity_id"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| args["activity_id"].as_i64().map(|id| id.to_string()))
                .unwrap_or_else(|| "latest".to_string());
            if let Some(act_ref) = resolve_activity_ref(&raw_id, activities) {
                let act = db
                    .and_then(|d| d.get_activity(act_ref.garmin_id).ok())
                    .flatten()
                    .unwrap_or_else(|| act_ref.clone());
                let track = db.and_then(|d| d.get_fit_track(act.garmin_id).ok()).flatten();
                let ctx = act.post_run_context.as_ref();
                let effective_temp_source = ctx
                    .map(|c| c.effective_temp_source(track.as_ref().map(|t| t.temp_source.as_str()).or(act.fit_temp_source.as_deref())))
                    .unwrap_or_else(|| {
                        track
                            .as_ref()
                            .map(|t| t.temp_source.clone())
                            .or_else(|| act.fit_temp_source.clone())
                            .unwrap_or_else(default_temp_source)
                    });
                let weights = db.and_then(|d| d.list_weight_entries(60).ok()).unwrap_or_default();
                let windows = scale_matcher::activity_windows(activities);
                let cfg = store.load_athlete_config().ok().flatten().unwrap_or_default();
                let ref_epoch = scale_matcher::parse_activity_epoch_sec(&act.start_iso)
                    .unwrap_or_else(|| Utc::now().timestamp());
                let resolved_weight = scale_matcher::resolve_effective_weight(
                    ctx,
                    ref_epoch,
                    &weights,
                    &windows,
                    cfg.specs.weight_kg,
                );
                let mut forensic = track.as_ref().and_then(|t| t.forensic_telemetry.clone());
                if let Some(ref mut f) = forensic {
                    f.temp_source = effective_temp_source.clone();
                }

                json!({
                    "activity": act,
                    "is_hyrox": act.is_hyrox,
                    "post_run_context": act.post_run_context,
                    "sweat_status": ctx.and_then(|c| c.sweat_status.as_deref()),
                    "provisional_sweat_rate_l_hr": ctx.and_then(|c| c.provisional_sweat_rate_l_hr),
                    "final_sweat_rate_l_hr": ctx.and_then(|c| c.final_sweat_rate_l_hr),
                    "effective_weight_kg": resolved_weight.as_ref().map(|w| w.weight_kg),
                    "weight_source": resolved_weight.as_ref().map(|w| w.source.as_str()),
                    "is_weight_provisional": resolved_weight.as_ref().map(|w| w.is_provisional),
                    "temp_source": effective_temp_source,
                    "fit_track_cached": track.is_some(),
                    "splits": track.as_ref().map(|t| &t.splits),
                    "laps": track.as_ref().map(|t| &t.laps),
                    "compromised_run_audit": track.as_ref().and_then(|t| t.compromised_run_audit.as_ref()),
                    "hyrox_dissection": track.as_ref().and_then(|t| t.hyrox_dissection.as_ref()),
                    "forensic_telemetry": forensic,
                    "decoupling_pct": track.as_ref().and_then(|t| t.decoupling_pct),
                    "guardrail_audit": track.as_ref().map(|t| &t.audit),
                })
            } else {
                json!({ "error": format!("Activity '{raw_id}' not found in synced store.") })
            }
        }
        "get_recent_activities" => {
            let limit = args["limit"].as_u64().unwrap_or(15).min(50) as usize;
            let slice: Vec<&ActivityRecord> = activities.iter().take(limit).collect();
            json!({ "count": slice.len(), "activities": slice })
        }
        "get_wellness_history" => {
            let count = args["days"].as_u64().unwrap_or(14).min(60) as usize;
            let slice: Vec<&DayRecord> = days.iter().take(count).collect();
            json!({ "count": slice.len(), "days": slice })
        }
        "get_readiness_and_plan" => {
            let config = store.load_athlete_config().ok().flatten().unwrap_or_default();
            let plan = store.read_json::<PlanFile>("plan.json").ok().flatten();
            let gear = store.read_json::<GearInventoryFile>("gear_inventory.json").ok().flatten();
            let output = ReadinessEngine::evaluate(ReadinessInput {
                now: Utc::now(),
                athlete_config: &config,
                activities,
                days,
                plan: plan.as_ref(),
                gear_inventory: gear.as_ref(),
            });
            let _ = output.persist(store.root());
            json!({
                "execution_mode": "self_contained",
                "readiness_state": output.readiness_state,
                "today_adapted_target": output.authoritative_target,
                "plan": plan
            })
        }
        "get_lifetime_and_yearly_history" => {
            let config = store.load_athlete_config().ok().flatten().unwrap_or_default();
            let summary = analyze_training_history(activities, &config, Utc::now());
            json!({
                "summary": summary,
                "compact_markdown": compact_history_markdown(&summary)
            })
        }
        "fetch_1hz_window" => {
            let raw_id = args["activity_id"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| args["activity_id"].as_i64().map(|id| id.to_string()))
                .unwrap_or_else(|| "latest".to_string());
            let start_sec = args["start_second"].as_f64().unwrap_or(0.0);
            let end_sec = args["end_second"].as_f64().unwrap_or(60.0);
            let step_sec = args["step_seconds"].as_i64().unwrap_or(1).max(1) as usize;

            if let Some(act) = resolve_activity_ref(&raw_id, activities) {
                if let Some(Some(track)) = db.and_then(|d| d.get_fit_track(act.garmin_id).ok()) {
                    fetch_1hz_window_from_track(&track, start_sec, end_sec, step_sec)
                } else {
                    json!({ "error": format!("No cached 1Hz .fit track found for activity_id {}", act.garmin_id) })
                }
            } else {
                json!({ "error": format!("Activity '{raw_id}' not found.") })
            }
        }
        "append_training_ledger" => {
            let entry = args["entry_markdown"].as_str().unwrap_or("");
            match append_training_ledger_entry(store, entry) {
                Ok(lines) => json!({ "status": "appended", "file": "ASPI_Training_Log.txt", "total_lines": lines }),
                Err(e) => json!({ "error": e }),
            }
        }
        "update_post_run_context" => {
            let raw_id = args["activity_id"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| args["activity_id"].as_i64().map(|id| id.to_string()))
                .unwrap_or_else(|| "latest".to_string());
            let fluid_ml = args.get("fluid_consumed_ml").and_then(|v| v.as_f64());
            let tempe_used = args.get("tempe_sensor_used").and_then(|v| v.as_bool());
            if let (Some(database), Some(act)) = (db, resolve_activity_ref(&raw_id, activities)) {
                match apply_post_run_context_tool_update(database, act, fluid_ml, tempe_used) {
                    Ok(v) => v,
                    Err(e) => json!({ "error": e }),
                }
            } else {
                json!({ "error": format!("Could not update post_run_context for activity '{raw_id}'.") })
            }
        }
        _ => json!({ "status": "ok", "note": "Local summary returned", "count": activities.len() }),
    }
}

fn resolve_activity_ref<'a>(raw_id: &str, activities: &'a [ActivityRecord]) -> Option<&'a ActivityRecord> {
    let trimmed = raw_id.trim().to_lowercase();
    match trimmed.as_str() {
        "" | "latest" => activities.first(),
        "latest_run" => activities
            .iter()
            .find(|a| a.type_key.to_lowercase().contains("run"))
            .or_else(|| activities.first()),
        other => other
            .parse::<i64>()
            .ok()
            .and_then(|id| activities.iter().find(|a| a.garmin_id == id)),
    }
}

pub fn apply_post_run_context_tool_update(
    db: &Database,
    activity: &ActivityRecord,
    fluid_consumed_ml: Option<f64>,
    tempe_sensor_used: Option<bool>,
) -> Result<Value, String> {
    let latest_act = db
        .get_activity(activity.garmin_id)
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| activity.clone());
    let mut ctx = latest_act.post_run_context.clone().unwrap_or_default();
    if let Some(fluid) = fluid_consumed_ml {
        ctx.fluid_consumed_ml = Some(fluid.max(0.0));
        ctx.fluid_intake_confirmed = true;
    }
    if let Some(tempe) = tempe_sensor_used {
        ctx.tempe_sensor_used = Some(tempe);
    }
    let dur = if latest_act.moving_duration_sec > 0.0 {
        latest_act.moving_duration_sec
    } else {
        latest_act.duration_sec
    };
    let computed = ctx.with_computed_sweat(dur);
    db.save_post_run_context(latest_act.garmin_id, &computed)
        .map_err(|e| e.to_string())?;
    let track = db.get_fit_track(latest_act.garmin_id).ok().flatten();
    let eff_temp = computed.effective_temp_source(
        track
            .as_ref()
            .map(|t| t.temp_source.as_str())
            .or(latest_act.fit_temp_source.as_deref()),
    );
    Ok(json!({
        "status": "updated",
        "activity_id": latest_act.garmin_id,
        "fluid_consumed_ml": computed.fluid_consumed_ml,
        "fluid_intake_confirmed": computed.fluid_intake_confirmed,
        "sweat_status": computed.sweat_status,
        "provisional_sweat_rate_l_hr": computed.provisional_sweat_rate_l_hr,
        "final_sweat_rate_l_hr": computed.final_sweat_rate_l_hr,
        "tempe_sensor_used": computed.tempe_sensor_used,
        "temp_source": eff_temp,
    }))
}

fn parse_iso(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc));
    }
    None
}

fn format_duration(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as i32;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn format_minutes(minutes: f64) -> String {
    let total = minutes.round().max(0.0) as i32;
    let h = total / 60;
    let m = total % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

pub const MIN_HISTORY_DAYS_FOR_AUDIT: i64 = 14;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct YearStat {
    pub year: i32,
    pub run_km: f64,
    pub hours: f64,
    pub run_count: usize,
    pub activity_count: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LifetimeHistory {
    pub earliest_iso: Option<String>,
    pub total_activities: usize,
    pub total_run_km: f64,
    pub years: Vec<YearStat>,
    pub peak_7d_km: f64,
    pub peak_28d_km: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrailingAudit {
    pub weeks_analyzed: usize,
    pub avg_weekly_km: f64,
    pub avg_weekly_runs: f64,
    pub long_run_km_low: Option<f64>,
    pub long_run_km_high: Option<f64>,
    pub run_sessions: usize,
    pub hybrid_sessions: usize,
    pub run_to_hybrid_ratio: Option<f64>,
    pub consistency_pct: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrainingHistorySummary {
    pub generated_at: String,
    pub goal_type: GoalType,
    pub days_of_history: i64,
    pub has_enough_history_for_audit: bool,
    pub lifetime: LifetimeHistory,
    pub trailing: TrailingAudit,
}

fn is_run_type(type_key: &str) -> bool {
    type_key.to_lowercase().contains("run")
}

fn is_hybrid_type(type_key: &str) -> bool {
    let lower = type_key.to_lowercase();
    matches!(lower.as_str(), "strength_training" | "hiit" | "cardio" | "indoor_rowing")
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

pub fn analyze_training_history(
    activities: &[ActivityRecord],
    config: &AthleteConfig,
    now: DateTime<Utc>,
) -> TrainingHistorySummary {
    let mut past: Vec<(&ActivityRecord, DateTime<Utc>)> = activities
        .iter()
        .filter_map(|a| parse_iso(&a.start_iso).filter(|dt| *dt <= now).map(|dt| (a, dt)))
        .collect();
    past.sort_by_key(|(_, dt)| *dt);

    let earliest_dt = past.first().map(|(_, dt)| *dt);
    let days_of_history = earliest_dt.map_or(0, |e| (now - e).num_days().max(0));

    let runs: Vec<(&ActivityRecord, DateTime<Utc>)> = past
        .iter()
        .copied()
        .filter(|(a, _)| is_run_type(&a.type_key))
        .collect();

    let total_run_km = round1(runs.iter().map(|(a, _)| a.distance_km()).sum());

    let mut by_year: BTreeMap<i32, (f64, f64, usize, usize)> = BTreeMap::new();
    for (a, dt) in &past {
        let entry = by_year.entry(dt.year()).or_insert((0.0, 0.0, 0, 0));
        entry.1 += a.moving_duration_sec / 3600.0;
        entry.3 += 1;
        if is_run_type(&a.type_key) {
            entry.0 += a.distance_km();
            entry.2 += 1;
        }
    }
    let years: Vec<YearStat> = by_year
        .into_iter()
        .map(|(year, (run_km, hours, run_count, activity_count))| YearStat {
            year,
            run_km: round1(run_km),
            hours: round1(hours),
            run_count,
            activity_count,
        })
        .collect();

    // Rolling peak 7d & 28d across runs
    let mut peak_7d = 0.0_f64;
    let mut peak_28d = 0.0_f64;
    for (_, end_dt) in &runs {
        let w7_start = *end_dt - Duration::days(7);
        let w28_start = *end_dt - Duration::days(28);
        let s7: f64 = runs
            .iter()
            .filter(|(_, dt)| *dt >= w7_start && *dt <= *end_dt)
            .map(|(a, _)| a.distance_km())
            .sum();
        let s28: f64 = runs
            .iter()
            .filter(|(_, dt)| *dt >= w28_start && *dt <= *end_dt)
            .map(|(a, _)| a.distance_km())
            .sum();
        peak_7d = peak_7d.max(s7);
        peak_28d = peak_28d.max(s28);
    }

    // Trailing 365 days
    let cutoff_365 = now - Duration::days(365);
    let trailing_acts: Vec<(&ActivityRecord, DateTime<Utc>)> = past
        .iter()
        .copied()
        .filter(|(_, dt)| *dt >= cutoff_365)
        .collect();
    let trailing_runs: Vec<(&ActivityRecord, DateTime<Utc>)> = trailing_acts
        .iter()
        .copied()
        .filter(|(a, _)| is_run_type(&a.type_key))
        .collect();
    let run_sessions = trailing_runs.len();
    let hybrid_sessions = trailing_acts
        .iter()
        .filter(|(a, _)| is_hybrid_type(&a.type_key))
        .count();

    let weeks_analyzed = if days_of_history > 0 {
        ((days_of_history.min(365) as f64) / 7.0).ceil().max(1.0) as usize
    } else {
        0
    };
    let trailing_km: f64 = trailing_runs.iter().map(|(a, _)| a.distance_km()).sum();
    let avg_weekly_km = if weeks_analyzed > 0 {
        round1(trailing_km / weeks_analyzed as f64)
    } else {
        0.0
    };
    let avg_weekly_runs = if weeks_analyzed > 0 {
        round1(run_sessions as f64 / weeks_analyzed as f64)
    } else {
        0.0
    };

    let mut week_buckets: BTreeMap<(i32, u32), f64> = BTreeMap::new();
    for (a, dt) in &trailing_runs {
        let iso = dt.iso_week();
        *week_buckets.entry((iso.year(), iso.week())).or_insert(0.0) += a.distance_km();
    }
    let active_weeks = week_buckets.values().filter(|&&km| km > 0.0).count();
    let consistency_pct = if weeks_analyzed > 0 {
        round1((active_weeks as f64 / weeks_analyzed as f64) * 100.0).min(100.0)
    } else {
        0.0
    };

    let mut run_dists: Vec<f64> = trailing_runs.iter().map(|(a, _)| a.distance_km()).collect();
    run_dists.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let (long_run_km_low, long_run_km_high) = if run_dists.len() >= 4 {
        let top_start = (run_dists.len() * 3) / 4;
        let top = &run_dists[top_start..];
        (
            top.first().map(|v| round1(*v)),
            top.last().map(|v| round1(*v)),
        )
    } else {
        (None, None)
    };

    let run_to_hybrid_ratio = if hybrid_sessions > 0 {
        Some(round1(run_sessions as f64 / hybrid_sessions as f64))
    } else {
        None
    };

    TrainingHistorySummary {
        generated_at: now.to_rfc3339(),
        goal_type: config.objective.goal_type,
        days_of_history,
        has_enough_history_for_audit: days_of_history >= MIN_HISTORY_DAYS_FOR_AUDIT,
        lifetime: LifetimeHistory {
            earliest_iso: earliest_dt.map(|d| d.to_rfc3339()),
            total_activities: past.len(),
            total_run_km,
            years,
            peak_7d_km: round1(peak_7d),
            peak_28d_km: round1(peak_28d),
        },
        trailing: TrailingAudit {
            weeks_analyzed,
            avg_weekly_km,
            avg_weekly_runs,
            long_run_km_low,
            long_run_km_high,
            run_sessions,
            hybrid_sessions,
            run_to_hybrid_ratio,
            consistency_pct,
        },
    }
}

pub fn compact_history_markdown(summary: &TrainingHistorySummary) -> String {
    let mut lines = vec![
        "### Tier 1: Lifetime Training Overview".to_string(),
        format!(
            "- **Totals**: {} activities, {:.1} km running across {} year(s)",
            summary.lifetime.total_activities,
            summary.lifetime.total_run_km,
            summary.lifetime.years.len()
        ),
        format!(
            "- **Peak Volume**: {:.1} km (7d) · {:.1} km (28d)",
            summary.lifetime.peak_7d_km, summary.lifetime.peak_28d_km
        ),
    ];
    for y in &summary.lifetime.years {
        lines.push(format!(
            "  - {}: {:.1} km ({} runs, {} total activities, {:.1}h)",
            y.year, y.run_km, y.run_count, y.activity_count, y.hours
        ));
    }
    lines.push(format!(
        "### Tier 2: Trailing 365-Day Block Audit ({} weeks)",
        summary.trailing.weeks_analyzed
    ));
    lines.push(format!(
        "- **Weekly Baseline**: {:.1} km/wk · {:.1} runs/wk · Consistency {:.1}%",
        summary.trailing.avg_weekly_km,
        summary.trailing.avg_weekly_runs,
        summary.trailing.consistency_pct
    ));
    if summary.trailing.hybrid_sessions > 0 {
        lines.push(format!(
            "- **Hybrid Mix**: {} runs, {} strength/erg sessions",
            summary.trailing.run_sessions, summary.trailing.hybrid_sessions
        ));
    }
    lines.join("\n")
}

pub fn starter_coach_profile_markdown(config: &AthleteConfig, summary: &TrainingHistorySummary) -> String {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let name = config.specs.display_name.trim();
    let display_name = if name.is_empty() { "Athlete" } else { name };
    let vt1_line = config
        .specs
        .vt1_ceiling_bpm
        .map_or("- `TODO: add VT1 aerobic ceiling`".to_string(), |v| {
            format!("- **VT1 aerobic ceiling**: {v} bpm")
        });
    let rest_line = config
        .medical
        .min_rest_hours_between_runs
        .map_or("- **Min spacing**: 36 hours".to_string(), |v| {
            format!("- **Min spacing**: {v} hours between high-strain sessions")
        });

    format!(
        "# COACH PROFILE\n\n## Athlete Identity\n- **Athlete**: {display_name}\n- **Units**: Metric (km, min/km, bpm, kg)\n- **Objective**: {}\n\n## Lab Data & Heart Rate Zones\n{vt1_line}\n\n## Medical & Biomechanical Guardrails\n{rest_line}\n\n## AVAILABILITY & CONTEXT\n| key | value | valid_from | valid_until | source | last_confirmed |\n| --- | --- | --- | --- | --- | --- |\n| training_days | {} days/week | {today} | open | athlete_config | {today} |\n\n## Change History\n- `{today}`: Seeded starter COACH_PROFILE.md from AthleteConfig and {} days of history.\n",
        config.objective.goal_type.display_name(),
        config.availability.available_days.len(),
        summary.days_of_history
    )
}

pub fn starter_training_plan_markdown(config: &AthleteConfig, summary: &TrainingHistorySummary) -> String {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let baseline = if summary.has_enough_history_for_audit {
        format!(
            "- **Recent baseline (trailing 365 days)**: {:.1} km/week and {:.1} runs/week",
            summary.trailing.avg_weekly_km, summary.trailing.avg_weekly_runs
        )
    } else {
        "- **Recent baseline**: fewer than 14 days of synced history; start conservatively and re-baseline after two weeks.".to_string()
    };
    let race_line = match config.objective.race_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(rn) => format!("- **Target Event**: {rn}"),
        None => "- **Target Event**: Open / Base progression".to_string(),
    };

    format!(
        "# TRAINING PLAN\n\n## Current Phase & Baseline\n- **Objective**: {}\n{baseline}\n\n## Mesocycle & Block Structure\n- **Block Rhythm**: 3 build weeks followed by 1 deload week.\n\n## Race Calendar & Targets\n{race_line}\n\n## Change History\n- `{today}`: Seeded starter TRAINING_PLAN.md from synced history.\n",
        config.objective.goal_type.display_name()
    )
}

pub fn first_use_setup_instructions(summary: &TrainingHistorySummary) -> String {
    if summary.has_enough_history_for_audit {
        format!(
            "FIRST-USE COACH SETUP: COACH_PROFILE.md and/or TRAINING_PLAN.md do not exist yet. Call `get_lifetime_and_yearly_history`, review the athlete's {} days of synced history, and propose grounded starter `COACH_PROFILE.md` and `TRAINING_PLAN.md` documents via `propose_coach_update` using the standard `##` headings.",
            summary.days_of_history
        )
    } else {
        "FIRST-USE COACH SETUP: COACH_PROFILE.md and/or TRAINING_PLAN.md do not exist yet and fewer than 14 days of history are synced. Propose conservative starter `COACH_PROFILE.md` and `TRAINING_PLAN.md` documents via `propose_coach_update` seeded from ATHLETE CONFIG.".to_string()
    }
}

pub fn format_user_profile_context(profile: &UserProfile) -> String {
    let mut lines = vec![format!("- Athlete Discipline: {}", profile.athlete_type.as_str())];

    if matches!(profile.athlete_type, AthleteType::Runner | AthleteType::HybridBoth) {
        let r = &profile.runner_config;
        let mut r_parts = Vec::new();
        if let Some(ref dist) = r.primary_distance {
            r_parts.push(format!("Primary distance: {dist}"));
        }
        r_parts.push(format!("HR-first governor: {}", r.hr_first_governor));
        if let Some(pf) = r.pace_floor_sec_per_km {
            r_parts.push(format!("Pace floor: {}/km", AthleteConfig::format_pace(pf)));
        }
        if let Some([lo, hi]) = r.z2_range_bpm {
            r_parts.push(format!("Z2: {lo}-{hi} bpm"));
        }
        if let Some([lo, hi]) = r.z3_range_bpm {
            r_parts.push(format!("Z3: {lo}-{hi} bpm"));
        }
        if let Some(lthr) = r.lthr_bpm {
            r_parts.push(format!("LTHR: {lthr} bpm"));
        }
        lines.push(format!("- Runner Config: {}", r_parts.join(" · ")));
    }

    if matches!(profile.athlete_type, AthleteType::Hyrox | AthleteType::HybridBoth) {
        let h = &profile.hyrox_config;
        let mut h_parts = Vec::new();
        if let Some(ref div) = h.division {
            h_parts.push(format!("Division: {div}"));
        }
        h_parts.push(format!("Weights: {}", h.official_weights_summary()));
        if let Some(ref mode) = h.watch_logging_mode {
            h_parts.push(format!("Watch logging: {mode}"));
        }
        if let Some(t) = h.target_total_time_sec {
            h_parts.push(format!("Target finish: {}", format_duration(t as f64)));
        }
        if let Some(fresh) = h.fresh_1km_pace_sec {
            h_parts.push(format!("Fresh 1km: {}/km", AthleteConfig::format_pace(fresh)));
        }
        if let Some(comp) = h.compromised_1km_target_sec {
            h_parts.push(format!("Compromised 1km target: {}/km", AthleteConfig::format_pace(comp)));
        }
        if !h.weak_stations.is_empty() {
            h_parts.push(format!("Weak stations: {}", h.weak_stations.join(", ")));
        }
        lines.push(format!("- HYROX Config: {}", h_parts.join(" · ")));
        lines.push("- HYROX Analysis Rule: Use `hyrox_dissection` (`clean_run_pace_sec_per_km`, `clean_aerobic_efficiency`, `compromised_pace_decay_sec`, `station_hr_recovery_sec`) from `get_activity_detail` so stationary station laps do not pollute running pace or aerobic efficiency.".to_string());
    }

    lines.join("\n")
}

pub fn build_static_system_instruction(
    store: &SharedStore,
    config: &AthleteConfig,
    activities: &[ActivityRecord],
    user_profile: Option<&UserProfile>,
) -> String {
    let profile_md = store.read_text("COACH_PROFILE.md").ok().flatten().unwrap_or_default();
    let plan_md = store.read_text("TRAINING_PLAN.md").ok().flatten().unwrap_or_default();
    let needs_setup = profile_md.trim().is_empty() || plan_md.trim().is_empty();

    let mut sections = vec![
        "You are a Living Markdown training-planning coach inside Splits Coach. Use metric units (km, min/km, bpm, kg). Always use `propose_coach_update` for Markdown changes. Use `get_recent_activities`, `get_activity_detail`, `get_wellness_history`, `get_training_load_and_records`, `get_readiness_and_plan`, `get_lifetime_and_yearly_history`, `fetch_1hz_window`, `append_training_ledger`, `update_post_run_context`, and `query_run_slice` for drill-downs. Sweat Matrix Rule: when analyzing a run with `sweat_status == \"provisional_no_intake\"`, report the provisional sweat rate and ask if the user had any intra-run fluid intake, then call `update_post_run_context` once confirmed (`0` ml if none). Weight Fallback Rule: inspect `weight_source` — when set to `\"profile_baseline\"` or `\"latest_14d_weigh_in\"`, label derived sweat/weight metrics provisional. Hybrid Tempe Rule: when `temp_source == \"watch_internal\"` and temperature data exists, check the user's post-run message for any mention of a Garmin Tempe sensor and call `update_post_run_context(tempe_sensor_used: true)` -> `\"user_confirmed_tempe\"` if mentioned, or ask the user to confirm whether a Tempe sensor was worn before treating temperature spikes as external ambient heat rather than wrist skin warmth.".to_string(),
        format!("=== ATHLETE CONFIG ===\n{}", config.prompt_summary(None)),
    ];
    if let Some(profile) = user_profile {
        sections.push(format!("=== ATHLETE DISCIPLINE PROFILE ===\n{}", format_user_profile_context(profile)));
    }
    if !profile_md.trim().is_empty() {
        sections.push(format!("=== COACH_PROFILE.md ===\n{profile_md}"));
    }
    if !plan_md.trim().is_empty() {
        sections.push(format!("=== TRAINING_PLAN.md ===\n{plan_md}"));
    }
    if needs_setup {
        let summary = analyze_training_history(activities, config, Utc::now());
        sections.push(first_use_setup_instructions(&summary));
    }
    #[cfg(feature = "beta-coach")]
    sections.push(
        "BETA COACH PERSONA DIRECTIVE: Keep all physiological, heart-rate zone, decoupling, and pace calculations 100% rigorous and accurate, while delivering dry, deadpan, slightly snarky commentary on cardiac drift, skipped recovery days, or sluggish HYROX transitions.".to_string()
    );
    sections.join("\n\n")
}

pub fn append_forensic_section(
    packet: String,
    activity: Option<&ActivityRecord>,
    report: Option<&crate::forensic_extractor::ForensicTelemetryReport>,
    resolved_weight: Option<&ResolvedWeight>,
) -> String {
    let mut blocks: Vec<String> = Vec::new();
    if let Some(w) = resolved_weight {
        let prov = if w.is_provisional { ", provisional" } else { "" };
        blocks.push(format!(
            "## Weight Source Metadata\n- **Effective Weight**: {:.1} kg (`weight_source`: {}{})",
            w.weight_kg, w.source, prov
        ));
    }
    if let Some(rep) = report {
        let label = activity
            .map(|a| {
                let date = a.start_iso.chars().take(10).collect::<String>();
                if a.name.trim().is_empty() {
                    format!("{date} {}", a.type_key)
                } else {
                    format!("{date} {}", a.name.trim())
                }
            })
            .unwrap_or_else(|| "Latest Tracked Run".to_string());
        let mut rep_clone = rep.clone();
        if let Some(ctx) = activity.and_then(|a| a.post_run_context.as_ref()) {
            rep_clone.temp_source = ctx.effective_temp_source(Some(&rep.temp_source));
        }
        blocks.push(format!(
            "## 1Hz Forensic Telemetry ({label})\n{}",
            rep_clone.to_markdown_summary()
        ));
    }
    if let Some(ctx) = activity.and_then(|a| a.post_run_context.as_ref()) {
        let mut lines = vec!["## Post-Run Context & Sweat Matrix".to_string()];
        if !ctx.shoe_model.trim().is_empty() || !ctx.shoe_size.trim().is_empty() {
            lines.push(format!(
                "- **Shoe**: {} {}",
                ctx.shoe_model.trim(),
                ctx.shoe_size.trim()
            ).trim().to_string());
        }
        if let (Some(pre), Some(post)) = (ctx.pre_run_weight_kg, ctx.post_run_weight_kg) {
            let ws = ctx.weight_source.as_deref().unwrap_or("manual");
            lines.push(format!(
                "- **Weights**: Pre {pre:.2} kg -> Post {post:.2} kg (Fluid {} ml · confirmed={} · `weight_source`: {ws})",
                ctx.fluid_consumed_ml.map(|f| format!("{f:.0}")).unwrap_or_else(|| "0".to_string()),
                ctx.fluid_intake_confirmed
            ));
        }
        if let Some(ref status) = ctx.sweat_status {
            lines.push(format!("- **Sweat Status**: {status}"));
        }
        if let Some(prov) = ctx.provisional_sweat_rate_l_hr {
            lines.push(format!("- **Provisional Sweat Rate (pre - post)**: {prov:.2} L/hr"));
        }
        if let Some(fin) = ctx.final_sweat_rate_l_hr {
            lines.push(format!("- **Final Verified Sweat Rate**: {fin:.2} L/hr"));
        }
        lines.push(format!(
            "- **Effective Temp Source**: {}",
            ctx.effective_temp_source(activity.and_then(|a| a.fit_temp_source.as_deref()))
        ));
        let o = &ctx.orthopedic;
        if o.left_achilles > 0 || o.lumbar_sciatica > 0 || o.right_ankle > 0 || o.calves_soleus > 0 {
            lines.push(format!(
                "- **Orthopedic Check-In**: L.Achilles {}/10 · Lumbar/Sciatica {}/10 · R.Ankle {}/10 · Calves/Soleus {}/10",
                o.left_achilles, o.lumbar_sciatica, o.right_ankle, o.calves_soleus
            ));
        }
        if lines.len() > 1 {
            blocks.push(lines.join("\n"));
        }
    }
    if blocks.is_empty() {
        return packet;
    }
    let extra = blocks.join("\n\n");
    if let Some(idx) = packet.find("\n\n## Coaching Request") {
        let (head, tail) = packet.split_at(idx);
        format!("{head}\n\n{extra}{tail}")
    } else {
        format!("{packet}\n\n{extra}")
    }
}

pub fn fetch_1hz_window_from_track(
    track: &FitTrackAnalysis,
    start_second: f64,
    end_second: f64,
    step_seconds: usize,
) -> Value {
    let lo = start_second.min(end_second).max(0.0);
    let hi = start_second.max(end_second).max(lo);
    let step = step_seconds.max(1);
    let rows: Vec<Value> = track
        .full_points
        .iter()
        .filter(|p| p.timestamp_sec >= lo && p.timestamp_sec <= hi)
        .step_by(step)
        .take(300)
        .map(|p| {
            json!({
                "second": p.timestamp_sec,
                "distance_m": p.distance_m,
                "speed_mps": p.speed_mps,
                "pace_sec_per_km": p.pace_sec_per_km,
                "hr_bpm": p.heart_rate_bpm,
                "power_w": p.power_w,
                "cadence_spm": p.cadence_spm,
                "gct_ms": p.gct_ms,
                "stance_pct": p.stance_time_pct,
                "vert_osc_mm": p.vertical_oscillation_mm,
                "vert_ratio_pct": p.vertical_ratio_pct,
                "step_length_mm": p.step_length_mm,
                "stance_balance_left_pct": p.stance_balance_left_pct,
                "altitude_m": p.altitude_m,
                "temp_c": p.temperature_c,
                "respiration_brpm": p.respiration_brpm,
                "lat": p.latitude_deg,
                "lon": p.longitude_deg,
                "impact_load": p.impact_load,
            })
        })
        .collect();
    json!({
        "garmin_id": track.garmin_id,
        "start_second": lo,
        "end_second": hi,
        "step_seconds": step,
        "row_count": rows.len(),
        "rows": rows
    })
}

pub fn append_training_ledger_entry(
    store: &SharedStore,
    entry_markdown: &str,
) -> Result<usize, String> {
    let trimmed = entry_markdown.trim();
    if trimmed.is_empty() {
        return Err("Entry markdown cannot be empty.".to_string());
    }
    let existing = store
        .read_text("ASPI_Training_Log.txt")
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let updated = if existing.trim().is_empty() {
        format!("{trimmed}\n")
    } else {
        format!("{}\n\n{trimmed}\n", existing.trim_end())
    };
    store
        .write_text_atomic("ASPI_Training_Log.txt", &updated)
        .map_err(|e| e.to_string())?;
    Ok(updated.lines().count())
}
