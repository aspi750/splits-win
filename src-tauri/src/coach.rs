use crate::models::*;
use crate::readiness::{ReadinessEngine, ReadinessInput};
use crate::storage::{CredentialStore, SharedStore};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use reqwest::Client;
use serde_json::{json, Value};

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

pub async fn run_gemini_coach_turn(
    api_key: &str,
    system_instruction: &str,
    user_prompt: &str,
    store: &SharedStore,
    activities: &[ActivityRecord],
    days: &[DayRecord],
) -> Result<(String, Vec<CoachUpdateProposal>), String> {
    let client = Client::new();
    let endpoint = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{GEMINI_MODEL_ID}:generateContent?key={api_key}"
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

        let obj: Value = resp.json().await.map_err(|e| e.to_string())?;
        let parts = obj["candidates"][0]["content"]["parts"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut model_parts = Vec::new();
        let mut function_calls = Vec::new();

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
                    model_parts.push(part.clone());
                }
                if !is_thought {
                    if let Some(t) = part["text"].as_str() {
                        final_text.push_str(t);
                    }
                }
            }
        }

        if function_calls.is_empty() {
            break;
        }

        let mut response_parts = Vec::new();
        for (name, args) in function_calls {
            let tool_resp = execute_tool(&name, &args, store, activities, days, &mut proposals);
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
