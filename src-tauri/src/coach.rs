use crate::models::*;
use crate::readiness::{ReadinessEngine, ReadinessInput};
use crate::storage::{CredentialStore, Database, SharedStore};
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
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
        "propose_coach_update" => "Drafting profile/plan update…",
        "query_run_slice" => "Inspecting run kilometer slice…",
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
            let raw_id = args["activity_id"].as_str().unwrap_or("latest");
            let target_act = match raw_id {
                "latest" => activities.first(),
                "latest_run" => activities.iter().find(|a| a.type_key.to_lowercase().contains("run")),
                other => other.parse::<i64>().ok().and_then(|id| activities.iter().find(|a| a.garmin_id == id)),
            };
            if let Some(act) = target_act {
                let track = db.and_then(|d| d.get_fit_track(act.garmin_id).ok()).flatten();
                json!({
                    "activity": act,
                    "fit_track_cached": track.is_some(),
                    "splits": track.as_ref().map(|t| &t.splits),
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
        _ => json!({ "status": "ok", "note": "Local summary returned", "count": activities.len() }),
    }
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
