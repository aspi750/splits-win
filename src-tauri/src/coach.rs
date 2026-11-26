use crate::models::*;
use crate::readiness::{ReadinessEngine, ReadinessInput};
use crate::storage::{CredentialStore, Database, SharedStore};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, Utc};
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
        Self::build_markdown_with_weights(
            config,
            activities,
            days,
            &[],
            gear_inventory,
            readiness_state,
            draft_prompt,
            now,
        )
    }

    pub fn build_markdown_with_weights(
        config: &AthleteConfig,
        activities: &[ActivityRecord],
        days: &[DayRecord],
        weights: &[WeightEntry],
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
        let resolved_rhr = config.resolved_resting_hr_baseline(&rhr_samples, now);
        let effective_rhr = resolved_rhr.map(|r| r.bpm);

        // 1. Athlete Profile & Active Guardrails
        let mut profile_sec = format!(
            "## Athlete Profile & Active Guardrails\n{}",
            config.prompt_summary(effective_rhr)
        );
        let windows = scale_matcher::activity_windows(activities);
        if let Some(latest_weight) = scale_matcher::latest_baseline_weigh_in(weights, &windows) {
            let date_str = latest_weight.timestamp_iso.get(..10).unwrap_or("");
            let stale_tag = if scale_matcher::is_fresh_baseline(latest_weight, now.timestamp()) {
                ""
            } else {
                " (stale: >14d)"
            };
            profile_sec.push_str(&format!(
                "\n- Garmin Connect Weight (Authoritative): {:.1} kg ({date_str}, {}{stale_tag})",
                latest_weight.weight_kg, latest_weight.source
            ));
        }
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
        let rhr_source_tag = resolved_rhr
            .map(|r| format!(" [rhr_baseline_source: {}]", r.source))
            .unwrap_or_default();
        let rhr_line = match (sorted_days.first().and_then(|d| d.resting_hr), effective_rhr) {
            (Some(latest), Some(base)) => {
                let delta = latest - base;
                let sign = if delta >= 0 { "+" } else { "" };
                format!("latest {latest} bpm ({sign}{delta} vs {base} bpm baseline){rhr_source_tag}")
            }
            (Some(latest), None) => format!("latest {latest} bpm"),
            (None, Some(base)) => format!("baseline {base} bpm (no reading today){rhr_source_tag}"),
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

            let notes_lines: Vec<String> = recent_acts_for_notes(activities);
            if !notes_lines.is_empty() {
                let mut block = vec!["## Garmin Connect Activity Notes".to_string()];
                block.extend(notes_lines);
                sections.push(block.join("\n"));
            }
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

fn recent_acts_for_notes(activities: &[ActivityRecord]) -> Vec<String> {
    let mut recent: Vec<&ActivityRecord> = activities
        .iter()
        .filter(|a| a.normalized_notes().is_some())
        .collect();
    recent.sort_by(|a, b| b.start_iso.cmp(&a.start_iso));
    recent
        .into_iter()
        .take(5)
        .filter_map(|act| {
            let notes = act.normalized_notes()?;
            let date_str = act.start_iso.chars().take(10).collect::<String>();
            let title = if act.name.trim().is_empty() {
                act.type_key.as_str()
            } else {
                act.name.trim()
            };
            Some(format!("- **{date_str} ({title})**: {notes}"))
        })
        .collect()
}

pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(((input.len() + 2) / 3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = *chunk.get(1).unwrap_or(&0) as usize;
        let b2 = *chunk.get(2).unwrap_or(&0) as usize;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) & 0x3F] as char);
        out.push(TABLE[(triple >> 12) & 0x3F] as char);
        out.push(if chunk.len() > 1 { TABLE[(triple >> 6) & 0x3F] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[triple & 0x3F] as char } else { '=' });
    }
    out
}

pub fn consolidate_proposals(proposals: Vec<CoachUpdateProposal>) -> Vec<CoachUpdateProposal> {
    let mut order: Vec<String> = Vec::new();
    let mut grouped: std::collections::HashMap<String, CoachUpdateProposal> =
        std::collections::HashMap::new();

    for prop in proposals {
        let key = prop.target_file.clone();
        if let Some(existing) = grouped.get_mut(&key) {
            let r_new = prop.reason.trim();
            if !r_new.is_empty()
                && !existing
                    .reason
                    .split(" · ")
                    .any(|part| part.trim() == r_new)
            {
                if existing.reason.trim().is_empty() {
                    existing.reason = r_new.to_string();
                } else {
                    existing.reason = format!("{} · {}", existing.reason.trim(), r_new);
                }
            }
            let next_md = prop.updated_section_markdown.trim();
            if !next_md.is_empty() && !existing.updated_section_markdown.contains(next_md) {
                if existing.updated_section_markdown.trim().is_empty() {
                    existing.updated_section_markdown = next_md.to_string();
                } else {
                    existing.updated_section_markdown = format!(
                        "{}\n\n{next_md}",
                        existing.updated_section_markdown.trim_end()
                    );
                }
            }
            existing.diff_preview = existing.updated_section_markdown.clone();
            if existing.expires_on.is_none() {
                existing.expires_on = prop.expires_on;
            }
            if prop.plan_json.is_some() {
                existing.plan_json = prop.plan_json;
            }
            if let Some(incoming_t) = prop.profile_thresholds {
                existing.profile_thresholds = Some(match existing.profile_thresholds.as_ref() {
                    Some(prev) => incoming_t.merged(prev),
                    None => incoming_t,
                });
            }
        } else {
            order.push(key.clone());
            let mut normalized = prop;
            normalized.diff_preview = normalized.updated_section_markdown.clone();
            grouped.insert(key, normalized);
        }
    }

    order.into_iter().filter_map(|k| grouped.remove(&k)).collect()
}

pub fn build_gemini_verify_url(api_key: &str, model_id: &str) -> String {
    let _trimmed = api_key.trim();
    let model = if model_id.trim().is_empty() {
        GEMINI_MODEL_ID
    } else {
        model_id.trim()
    };
    format!("https://generativelanguage.googleapis.com/v1beta/models/{model}")
}

pub fn classify_gemini_verify_status(status: u16, body: &str) -> Result<String, String> {
    if (200..=299).contains(&status) {
        let model_name = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| {
                v["displayName"]
                    .as_str()
                    .or_else(|| v["name"].as_str())
                    .map(str::to_string)
            })
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Gemini 4 Hydrogen".to_string());
        let label = if model_name.to_lowercase().contains("hydrogen") {
            "Gemini 4 Hydrogen".to_string()
        } else {
            model_name
        };
        return Ok(format!("Connected ({label} verified · HTTP {status})"));
    }

    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::trim).map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or_default();

    if status == 400 || status == 403 {
        return Err(if detail.is_empty() {
            format!("HTTP {status} Invalid API Key")
        } else {
            format!("HTTP {status} Invalid API Key: {detail}")
        });
    }
    if status == 429 {
        return Err(if detail.is_empty() {
            "HTTP 429 Quota Reached".to_string()
        } else {
            format!("HTTP 429 Quota Reached: {detail}")
        });
    }
    Err(if detail.is_empty() {
        format!("HTTP {status} error from Gemini API")
    } else {
        format!("HTTP {status}: {detail}")
    })
}

pub async fn verify_gemini_api_key(api_key: &str) -> Result<String, String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err("Gemini API key cannot be empty.".to_string());
    }
    let url = build_gemini_verify_url(trimmed, GEMINI_MODEL_ID);
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("x-goog-api-key", trimmed)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    classify_gemini_verify_status(status, &body)
}

pub fn resolve_coach_engine_mode() -> CoachEngineMode {
    if CredentialStore::read_secret(APP_SERVICE, GEMINI_ACCOUNT).is_some() {
        CoachEngineMode::GeminiApi
    } else {
        CoachEngineMode::ClipboardPacket
    }
}

fn replace_single_section(markdown: &str, trimmed: &str) -> String {
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

pub fn replacing_section(markdown: &str, section_markdown: &str) -> String {
    let trimmed = section_markdown.trim();
    if trimmed.is_empty() {
        return if markdown.is_empty() {
            String::new()
        } else {
            format!("{}\n", markdown.trim_end())
        };
    }

    if trimmed.starts_with("# ") {
        return format!("{trimmed}\n");
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    let heading_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(idx, l)| if l.starts_with("## ") { Some(idx) } else { None })
        .collect();

    if heading_indices.len() <= 1 {
        return replace_single_section(markdown, trimmed);
    }

    let mut current = markdown.to_string();
    for (pos, &start) in heading_indices.iter().enumerate() {
        let end = heading_indices
            .get(pos + 1)
            .copied()
            .unwrap_or(lines.len());
        let chunk = lines[start..end].join("\n");
        current = replace_single_section(&current, chunk.trim());
    }
    current
}

pub fn gemini_tool_declarations() -> Value {
    json!([
        {
            "name": "propose_coach_update",
            "description": "Propose a reversible consolidated Markdown update to COACH_PROFILE.md or TRAINING_PLAN.md (with optional plan_json for TRAINING_PLAN.md).",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "target_file": { "type": "STRING" },
                    "reason": { "type": "STRING" },
                    "updated_section_markdown": { "type": "STRING" },
                    "expires_on": { "type": "STRING" },
                    "plan_json": { "type": "OBJECT" },
                    "profile_thresholds": {
                        "type": "OBJECT",
                        "description": "Structured athlete thresholds for COACH_PROFILE.md updates (vt1_bpm, vt2_bpm, vt1_pace_sec_per_km, vt2_pace_sec_per_km, lthr_bpm, max_hr_bpm, resting_hr_bpm, z2_range_bpm, z3_range_bpm, weight_kg, primary_distance, hr_first_governor, hard_pace_floor_sec_per_km)."
                    }
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
    if proposal.target_file == "TRAINING_PLAN.md" {
        if let Some(ref plan) = proposal.plan_json {
            store
                .write_json_atomic("plan.json", plan)
                .map_err(|e| e.to_string())?;
        }
    }
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
    model_settings: &ModelSettings,
    intake_files: &[IntakeQueuedFile],
) -> Result<(String, Vec<CoachUpdateProposal>), String> {
    let resolved = resolve_model(ModelScope::Coach, model_settings);
    let client = Client::new();
    let endpoint = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse",
        resolved.model_id
    );

    let canon_root = std::fs::canonicalize(store.root()).ok();
    let mut initial_parts: Vec<Value> = vec![json!({ "text": user_prompt })];
    for f in intake_files {
        let rel = f.relative_path.trim().trim_start_matches(['/', '\\']);
        if rel.is_empty() || rel.contains("..") {
            continue;
        }
        let full_path = store.root().join(rel);
        if !full_path.exists() {
            continue;
        }
        if let Some(ref root) = canon_root {
            let Ok(canon_file) = std::fs::canonicalize(&full_path) else {
                continue;
            };
            if !canon_file.starts_with(root) {
                continue;
            }
        }
        let lower_name = f.file_name.to_lowercase();
        if lower_name.ends_with(".pdf") || f.mime_type == "application/pdf" {
            if let Ok(bytes) = std::fs::read(&full_path) {
                if bytes.len() <= 15 * 1024 * 1024 {
                    initial_parts.push(json!({
                        "inlineData": {
                            "mimeType": "application/pdf",
                            "data": base64_encode(&bytes)
                        }
                    }));
                } else {
                    initial_parts.push(json!({
                        "text": format!("Note: Uploaded PDF '{}' exceeds 15 MB inline limit.", f.file_name)
                    }));
                }
            }
        } else if lower_name.ends_with(".fit") {
            let summary = match crate::garmin::summarize_fit_file_for_intake(&full_path) {
                Ok(s) => s,
                Err(e) => format!("Failed to parse {}: {e}", f.file_name),
            };
            initial_parts.push(json!({
                "text": format!("=== Uploaded Garmin FIT Intake ({}) ===\n{summary}", f.file_name)
            }));
        } else if let Ok(raw) = std::fs::read_to_string(&full_path) {
            let snippet: String = raw.chars().take(16_000).collect();
            initial_parts.push(json!({
                "text": format!("=== Uploaded Intake Notes ({}) ===\n{snippet}", f.file_name)
            }));
        }
    }

    let mut contents = vec![json!({
        "role": "user",
        "parts": initial_parts
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
                "thinkingConfig": { "thinkingLevel": resolved.thinking_level.as_str() }
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
                        profile: "COACH".to_string(),
                        model: resolved.model_id.clone(),
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

    let consolidated = consolidate_proposals(proposals);
    Ok((final_text, consolidated))
}

pub fn build_activity_detail_tool_payload(
    act: &ActivityRecord,
    store: &SharedStore,
    db: Option<&Database>,
    activities: &[ActivityRecord],
) -> Value {
    let track = db.and_then(|d| d.get_fit_track(act.garmin_id).ok()).flatten();
    let notes = act.normalized_notes().map(str::to_string);
    let mut ctx_owned = act.post_run_context.clone();
    if let Some(ref n) = notes {
        let c = ctx_owned.get_or_insert_with(PostRunContext::default);
        if c.garmin_notes.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_none() {
            c.garmin_notes = Some(n.clone());
        }
    }
    let ctx = ctx_owned.as_ref();
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
        f.garmin_notes = notes.clone();
    }

    json!({
        "activity": act,
        "garmin_notes": notes,
        "is_hyrox": act.is_hyrox,
        "post_run_context": ctx_owned,
        "sweat_status": ctx.and_then(|c| c.sweat_status.as_deref()),
        "net_fluid_deficit_kg": ctx.and_then(|c| c.net_fluid_deficit_kg),
        "provisional_sweat_rate_l_hr": ctx.and_then(|c| c.provisional_sweat_rate_l_hr),
        "final_sweat_rate_l_hr": ctx.and_then(|c| c.final_sweat_rate_l_hr),
        "pre_weigh_in_offset_min": ctx.and_then(|c| c.pre_weigh_in_offset_min),
        "post_weigh_in_offset_min": ctx.and_then(|c| c.post_weigh_in_offset_min),
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
}

pub fn build_garmin_weight_json(entry: &WeightEntry, now_epoch: i64) -> Value {
    let date_str = entry
        .timestamp_iso
        .get(..10)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            chrono::DateTime::<Utc>::from_timestamp(entry.timestamp_epoch_sec, 0)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        });
    let mut obj = json!({
        "weight_kg": entry.weight_kg,
        "date": date_str,
        "timestamp_iso": entry.timestamp_iso,
        "source": entry.source,
        "is_fresh": scale_matcher::is_fresh_baseline(entry, now_epoch),
        "authoritative": true,
    });
    if let Some(bf) = entry.body_fat_pct {
        obj["body_fat_pct"] = json!(bf);
    }
    if let Some(mm) = entry.muscle_mass_kg {
        obj["muscle_mass_kg"] = json!(mm);
    }
    obj
}

pub fn build_wellness_history_tool_payload(
    config: &AthleteConfig,
    days: &[DayRecord],
    weights: &[WeightEntry],
    activities: &[ActivityRecord],
    requested_days: usize,
    now_epoch: i64,
) -> Value {
    let count = requested_days.clamp(1, 60);
    let slice: Vec<DayRecord> = days
        .iter()
        .take(count)
        .map(|d| {
            let mut slim = d.clone();
            slim.body_battery_series = None;
            slim
        })
        .collect();
    let ref_dt = chrono::DateTime::<Utc>::from_timestamp(now_epoch, 0).unwrap_or_else(Utc::now);
    let rhr_samples: Vec<(DateTime<Utc>, Option<i32>)> = days
        .iter()
        .filter_map(|d| {
            NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")
                .ok()
                .and_then(|nd| nd.and_hms_opt(12, 0, 0))
                .map(|ndt| (DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc), d.resting_hr))
        })
        .collect();
    let resolved_rhr = config.resolved_resting_hr_baseline(&rhr_samples, ref_dt);
    let windows = scale_matcher::activity_windows(activities);
    let latest_garmin_weight = scale_matcher::latest_baseline_weigh_in(weights, &windows)
        .map(|w| build_garmin_weight_json(w, now_epoch));
    let recent_weight_entries: Vec<Value> = weights
        .iter()
        .take(14)
        .map(|entry| {
            let date_str = entry
                .timestamp_iso
                .get(..10)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    chrono::DateTime::<Utc>::from_timestamp(entry.timestamp_epoch_sec, 0)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                        .unwrap_or_default()
                });
            let mut row = json!({
                "date": date_str,
                "timestamp_iso": entry.timestamp_iso,
                "weight_kg": entry.weight_kg,
                "source": entry.source,
                "is_post_run_dehydrated": scale_matcher::is_post_run_dehydrated(entry.timestamp_epoch_sec, &windows),
            });
            if let Some(bf) = entry.body_fat_pct {
                row["body_fat_pct"] = json!(bf);
            }
            if let Some(mm) = entry.muscle_mass_kg {
                row["muscle_mass_kg"] = json!(mm);
            }
            row
        })
        .collect();

    let mut payload = json!({
        "count": slice.len(),
        "days": slice,
    });
    if let Some(r) = resolved_rhr {
        payload["baseline_rhr_bpm"] = json!(r.bpm);
        payload["rhr_baseline_source"] = json!(r.source);
    }
    if let Some(latest) = latest_garmin_weight {
        payload["latest_garmin_weight"] = latest;
    }
    if !recent_weight_entries.is_empty() {
        payload["recent_weight_entries"] = json!(recent_weight_entries);
    }
    payload
}

pub fn build_readiness_and_plan_tool_payload(
    store: &SharedStore,
    activities: &[ActivityRecord],
    days: &[DayRecord],
    weights: &[WeightEntry],
    now: DateTime<Utc>,
) -> Value {
    let config = store.load_athlete_config().ok().flatten().unwrap_or_default();
    let today_str = now.format("%Y-%m-%d").to_string();
    let plan = store
        .load_plan_with_auto_rollover(&today_str, &now.to_rfc3339())
        .ok()
        .flatten();
    let gear = store.read_json::<GearInventoryFile>("gear_inventory.json").ok().flatten();
    let rhr_samples: Vec<(DateTime<Utc>, Option<i32>)> = days
        .iter()
        .filter_map(|d| {
            NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")
                .ok()
                .and_then(|nd| nd.and_hms_opt(12, 0, 0))
                .map(|ndt| (DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc), d.resting_hr))
        })
        .collect();
    let resolved_rhr = config.resolved_resting_hr_baseline(&rhr_samples, now);
    let output = ReadinessEngine::evaluate(ReadinessInput {
        now,
        athlete_config: &config,
        activities,
        days,
        plan: plan.as_ref(),
        gear_inventory: gear.as_ref(),
    });
    let _ = output.persist(store.root());
    let windows = scale_matcher::activity_windows(activities);
    let latest_garmin_weight = scale_matcher::latest_baseline_weigh_in(weights, &windows)
        .map(|w| build_garmin_weight_json(w, now.timestamp()));
    let mut payload = json!({
        "execution_mode": "self_contained",
        "readiness_state": output.readiness_state,
        "today_adapted_target": output.authoritative_target,
        "plan": plan
    });
    if let Some(r) = resolved_rhr {
        payload["baseline_rhr_bpm"] = json!(r.bpm);
        payload["rhr_baseline_source"] = json!(r.source);
    }
    if let Some(latest) = latest_garmin_weight {
        payload["latest_garmin_weight"] = latest;
    }
    payload
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
            let plan_json: Option<PlanFile> = args
                .get("plan_json")
                .filter(|v| !v.is_null())
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let parsed_thresholds: Option<CoachProfileThresholds> = args
                .get("profile_thresholds")
                .and_then(|v| serde_json::from_value::<CoachProfileThresholds>(v.clone()).ok());
            let md_thresholds = parse_profile_thresholds_from_markdown(&section);
            let combined_thresholds = match parsed_thresholds {
                Some(t) => {
                    let m = t.merged(&md_thresholds);
                    if m.is_empty() { None } else { Some(m) }
                }
                None if !md_thresholds.is_empty() => Some(md_thresholds),
                None => None,
            };
            let id = uuid::Uuid::new_v4().to_string();
            proposals.push(CoachUpdateProposal {
                id: id.clone(),
                target_file: target.clone(),
                reason,
                updated_section_markdown: section.clone(),
                expires_on: expires,
                diff_preview: section,
                status: "pending_user_approval".to_string(),
                plan_json,
                profile_thresholds: combined_thresholds,
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
                build_activity_detail_tool_payload(&act, store, db, activities)
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
            let config = store.load_athlete_config().ok().flatten().unwrap_or_default();
            let weights = db.and_then(|d| d.list_weight_entries(60).ok()).unwrap_or_default();
            build_wellness_history_tool_payload(&config, days, &weights, activities, count, Utc::now().timestamp())
        }
        "get_readiness_and_plan" => {
            let weights = db.and_then(|d| d.list_weight_entries(60).ok()).unwrap_or_default();
            build_readiness_and_plan_tool_payload(store, activities, days, &weights, Utc::now())
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
    let target_line = config
        .objective
        .target_time_description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|t| format!("\n- **Goal Time**: {t}"))
        .unwrap_or_default();

    format!(
        "# COACH PROFILE\n\n## Athlete Identity\n- **Athlete**: {display_name}\n- **Units**: Metric (km, min/km, bpm, kg)\n- **Objective**: {}{target_line}\n\n## Lab Data & Heart Rate Zones\n{vt1_line}\n\n## Medical & Biomechanical Guardrails\n{rest_line}\n\n## AVAILABILITY & CONTEXT\n| key | value | valid_from | valid_until | source | last_confirmed |\n| --- | --- | --- | --- | --- | --- |\n| training_days | {} days/week | {today} | open | athlete_config | {today} |\n\n## Change History\n- `{today}`: Seeded starter COACH_PROFILE.md from AthleteConfig and {} days of history.\n",
        config.objective.goal_type.display_name(),
        config.availability.available_days.len(),
        summary.days_of_history
    )
}

pub fn starter_training_plan_markdown(config: &AthleteConfig, summary: &TrainingHistorySummary) -> String {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let goal_type = config.objective.goal_type;
    let baseline = if summary.has_enough_history_for_audit {
        format!(
            "- **Recent baseline (trailing 365 days)**: {:.1} km/week and {:.1} runs/week",
            summary.trailing.avg_weekly_km, summary.trailing.avg_weekly_runs
        )
    } else {
        "- **Recent baseline**: fewer than 14 days of synced history; start conservatively and re-baseline after two weeks.".to_string()
    };

    let block_rhythm = if goal_type == GoalType::CouchTo5k {
        "- **Block Rhythm**: 8-9 week Couch to 5K (C25K) run-walk progression targeting a continuous 5.0 km / 30 min run (3 sessions per week with 48h rest spacing)."
    } else if goal_type.is_reentry_or_starter() {
        "- **Block Rhythm**: Conservative aerobic re-entry wave using short run-walk intervals (48h rest spacing between runs; zero quality speedwork until durable)."
    } else {
        "- **Block Rhythm**: 3 build weeks followed by 1 deload week."
    };

    let trimmed_name = config.objective.race_name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let valid_dist = config.objective.race_distance_km.filter(|&d| d > 0.0);
    let trimmed_date = config.objective.race_date.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let trimmed_goal = config
        .objective
        .target_time_description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let targets_section = if trimmed_name.is_some() || valid_dist.is_some() || trimmed_date.is_some() {
        let mut line = format!("- **Target Event**: {}", trimmed_name.unwrap_or(goal_type.display_name()));
        if let Some(d) = valid_dist {
            line.push_str(&format!(" ({d:.1} km)"));
        }
        if let Some(date) = trimmed_date {
            line.push_str(&format!(" on {date}"));
        }
        if let Some(t) = trimmed_goal {
            line.push_str(&format!("\n- **Goal Time**: {t}"));
        } else {
            line.push_str(" (finish goal — no target time set)");
        }
        line
    } else if let Some(t) = trimmed_goal {
        format!("- **Target Event**: Open / Base progression\n- **Goal Time**: {t}")
    } else {
        "- **Target Event**: Open / Base progression".to_string()
    };

    format!(
        "# TRAINING PLAN\n\n## Current Phase & Baseline\n- **Objective**: {}\n{baseline}\n\n## Mesocycle & Block Structure\n{block_rhythm}\n\n## Race Calendar & Targets\n{targets_section}\n\n## Change History\n- `{today}`: Seeded starter TRAINING_PLAN.md from synced history.\n",
        goal_type.display_name()
    )
}

pub fn starter_plan_file(config: &AthleteConfig, summary: &TrainingHistorySummary) -> PlanFile {
    let today = Local::now().date_naive();
    let from = today.format("%Y-%m-%d").to_string();
    let to = (today + Duration::days(6)).format("%Y-%m-%d").to_string();
    let goal_type = config.objective.goal_type;

    let base_weekly = if goal_type == GoalType::CouchTo5k {
        12.0
    } else if goal_type.is_reentry_or_starter() {
        if summary.trailing.avg_weekly_km > 4.0 {
            (summary.trailing.avg_weekly_km * 0.6).clamp(8.0, 18.0).round()
        } else {
            12.0
        }
    } else if summary.trailing.avg_weekly_km > 5.0 {
        summary.trailing.avg_weekly_km.round()
    } else {
        15.0
    };

    let max_single_km = if goal_type == GoalType::CouchTo5k {
        5.0
    } else {
        let min_cap = if goal_type.is_reentry_or_starter() { 3.5 } else { 5.0 };
        (base_weekly * 0.35).clamp(min_cap, 21.0)
    };

    let phase = if goal_type == GoalType::CouchTo5k {
        "couch_to_5k"
    } else if goal_type.is_reentry_or_starter() {
        "aerobic_reentry"
    } else {
        "base"
    };

    let min_hours = config
        .medical
        .min_rest_hours_between_runs
        .unwrap_or(if goal_type.is_reentry_or_starter() { 48 } else { 36 })
        .max(12) as f64;
    let hr_cap = config
        .medical
        .easy_hr_ceiling_bpm
        .or(config.specs.vt1_ceiling_bpm)
        .unwrap_or(145);

    let avail_days: Vec<i32> = if config.availability.available_days.is_empty() {
        vec![2, 4, 7]
    } else {
        config.availability.available_days.clone()
    };
    let long_day = config.availability.preferred_long_run_day.unwrap_or(7);
    let min_day_gap = ((min_hours / 24.0).ceil() as i64).max(1);
    let mut sessions = Vec::new();
    let mut last_offset: Option<i64> = None;

    for offset in 0..7i64 {
        let day = today + Duration::days(offset);
        let iso = day.weekday().number_from_monday() as i32;
        if !avail_days.contains(&iso) {
            continue;
        }
        if let Some(prev) = last_offset {
            if offset - prev < min_day_gap {
                continue;
            }
        }
        let is_long = iso == long_day;
        let typology = if goal_type.is_reentry_or_starter() {
            RunTypology::RunWalkReentry
        } else if is_long {
            RunTypology::LongRun
        } else {
            RunTypology::EasyAerobic
        };
        let dist = if goal_type == GoalType::CouchTo5k {
            ((base_weekly / 3.5).max(2.5).min(max_single_km) * 10.0).round() / 10.0
        } else if is_long {
            (max_single_km * 10.0).round() / 10.0
        } else {
            ((base_weekly / 3.5).max(3.0) * 10.0).round() / 10.0
        };
        let dur_min = ((dist * 7.5) * 10.0).round() / 10.0;
        let date_str = day.format("%Y-%m-%d").to_string();
        sessions.push(PlanSession {
            id: format!("s-{date_str}"),
            date: date_str,
            session_type: typology,
            target: PlanTargetRange {
                duration_min: vec![(dur_min - 5.0).max(20.0), dur_min + 5.0],
                distance_km: vec![(dist - 0.5).max(2.0), dist + 0.5],
            },
            hr_cap_bpm: hr_cap,
            shoe_constraints: Vec::new(),
            alternatives: vec!["30 min easy walk or mobility".to_string()],
        });
        last_offset = Some(offset);
        if sessions.len() >= 3 {
            break;
        }
    }

    PlanFile {
        version: 1,
        revision: 1,
        updated_at: Utc::now().to_rfc3339(),
        updated_by: "splits_coach".to_string(),
        covers: PlanDateRange { from, to },
        status: PlanStatus::Valid,
        envelope: PlanEnvelope {
            max_weekly_km: base_weekly,
            max_single_run_km: max_single_km,
            mesocycle_phase: phase.to_string(),
            min_hours_between_runs: min_hours,
            walk_hr_cap_bpm: Some((hr_cap - 15).max(100)),
        },
        sessions,
    }
}

pub fn auto_calibrate_thresholds_from_history(
    config: &mut AthleteConfig,
    user_profile: &mut UserProfile,
    activities: &[ActivityRecord],
    _days: &[DayRecord],
    _now: DateTime<Utc>,
) -> bool {
    let mut changed = false;

    // Keep specs.resting_hr_baseline as None so the trailing 30-day median tracks automatically
    // unless the athlete explicitly opts into a manual override.

    if config.specs.max_hr.is_none() {
        if let Some(observed_max) = activities
            .iter()
            .filter_map(|a| a.max_hr)
            .filter(|&h| (130.0..=230.0).contains(&h))
            .fold(None::<f64>, |acc, h| Some(acc.map_or(h, |m| m.max(h))))
        {
            config.specs.max_hr = Some(observed_max.round() as i32);
            changed = true;
        }
    }

    if let Some(max_hr) = config.specs.max_hr.filter(|&m| m > 100) {
        if user_profile.runner_config.lthr_bpm.is_none() {
            user_profile.runner_config.lthr_bpm = Some((max_hr as f64 * 0.89).round() as i32);
            changed = true;
        }
        if config.specs.vt1_ceiling_bpm.is_none() {
            config.specs.vt1_ceiling_bpm = Some((max_hr as f64 * 0.78).round() as i32);
            changed = true;
        }
    }

    changed
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
        if let Some(vt1) = r.vt1_bpm {
            let pace_txt = r
                .vt1_pace_sec_per_km
                .map(|p| format!(" ({}/km)", AthleteConfig::format_pace(p)))
                .unwrap_or_default();
            r_parts.push(format!("VT1: {vt1} bpm{pace_txt}"));
        }
        if let Some(vt2) = r.vt2_bpm {
            let pace_txt = r
                .vt2_pace_sec_per_km
                .map(|p| format!(" ({}/km)", AthleteConfig::format_pace(p)))
                .unwrap_or_default();
            r_parts.push(format!("VT2: {vt2} bpm{pace_txt}"));
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
        if let Some(mhr) = r.max_hr_bpm {
            r_parts.push(format!("Max HR: {mhr} bpm"));
        }
        if let Some(rhr) = r.resting_hr_bpm {
            r_parts.push(format!("Resting HR: {rhr} bpm"));
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

fn extract_first_bpm_in_line(line: &str) -> Option<i32> {
    let lower = line.to_lowercase();
    for (idx, _) in lower.match_indices("bpm") {
        let prefix = lower[..idx].trim_end();
        let digits: String = prefix
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if let Ok(v) = digits.parse::<i32>() {
            if (30..=230).contains(&v) {
                return Some(v);
            }
        }
    }
    if let Some(colon_idx) = line.find(':') {
        for tok in line[colon_idx + 1..].split(|c: char| !c.is_ascii_digit()) {
            if let Ok(v) = tok.parse::<i32>() {
                if (30..=230).contains(&v) {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn extract_pace_sec_in_line(line: &str) -> Option<i32> {
    for raw_tok in line.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';')) {
        let cleaned = raw_tok.trim_end_matches("/km").trim_end_matches("min");
        let parts: Vec<&str> = cleaned.split(':').collect();
        if parts.len() == 2 {
            if let (Ok(m), Ok(s)) = (parts[0].parse::<i32>(), parts[1].parse::<i32>()) {
                if (2..=20).contains(&m) && (0..60).contains(&s) && parts[1].len() == 2 {
                    return Some(m * 60 + s);
                }
            }
        }
    }
    None
}

fn extract_range_in_line(line: &str) -> Option<[i32; 2]> {
    let normalized = line.replace(['–', '—'], "-");
    for seg in normalized.split_whitespace() {
        if let Some((a, b)) = seg.split_once('-') {
            let a_digits: String = a.chars().filter(|c| c.is_ascii_digit()).collect();
            let b_digits: String = b.chars().filter(|c| c.is_ascii_digit()).collect();
            if let (Ok(lo), Ok(hi)) = (a_digits.parse::<i32>(), b_digits.parse::<i32>()) {
                if (60..=230).contains(&lo) && (lo..=230).contains(&hi) {
                    return Some([lo, hi]);
                }
            }
        }
    }
    None
}

fn extract_weight_kg_in_line(line: &str) -> Option<f64> {
    let lower = line.to_lowercase();
    for (idx, _) in lower.match_indices("kg") {
        let prefix = lower[..idx].trim_end();
        let num_str: String = prefix
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if let Ok(w) = num_str.parse::<f64>() {
            if (30.0..=220.0).contains(&w) {
                return Some(w);
            }
        }
    }
    None
}

pub fn parse_profile_thresholds_from_markdown(markdown: &str) -> CoachProfileThresholds {
    let mut out = CoachProfileThresholds::default();
    for raw in markdown.lines() {
        let cleaned = raw.replace('*', "").trim().to_string();
        if cleaned.is_empty() {
            continue;
        }
        let lower = cleaned.to_lowercase();
        if lower.contains("vt1") {
            if out.vt1_bpm.is_none() {
                out.vt1_bpm = extract_first_bpm_in_line(&cleaned);
            }
            if out.vt1_pace_sec_per_km.is_none() {
                out.vt1_pace_sec_per_km = extract_pace_sec_in_line(&cleaned);
            }
        } else if lower.contains("vt2") {
            if out.vt2_bpm.is_none() {
                out.vt2_bpm = extract_first_bpm_in_line(&cleaned);
            }
            if out.vt2_pace_sec_per_km.is_none() {
                out.vt2_pace_sec_per_km = extract_pace_sec_in_line(&cleaned);
            }
        }
        if out.lthr_bpm.is_none() && (lower.contains("lthr") || lower.contains("lactate threshold")) {
            out.lthr_bpm = extract_first_bpm_in_line(&cleaned);
        }
        if out.max_hr_bpm.is_none() && (lower.contains("max hr") || lower.contains("maximum heart rate")) {
            out.max_hr_bpm = extract_first_bpm_in_line(&cleaned);
        }
        if out.resting_hr_bpm.is_none() && (lower.contains("resting hr") || lower.contains("resting heart rate")) {
            out.resting_hr_bpm = extract_first_bpm_in_line(&cleaned);
        }
        if out.z2_range_bpm.is_none() && (lower.contains("z2") || lower.contains("zone 2")) {
            out.z2_range_bpm = extract_range_in_line(&cleaned);
        }
        if out.z3_range_bpm.is_none() && (lower.contains("z3") || lower.contains("zone 3")) {
            out.z3_range_bpm = extract_range_in_line(&cleaned);
        }
        if out.weight_kg.is_none() && (lower.contains("weight") || lower.contains("mass")) {
            out.weight_kg = extract_weight_kg_in_line(&cleaned);
        }
        if out.hard_pace_floor_sec_per_km.is_none()
            && (lower.contains("pace floor") || lower.contains("minimum pace"))
        {
            out.hard_pace_floor_sec_per_km = extract_pace_sec_in_line(&cleaned);
        }
        if out.hr_first_governor.is_none() && lower.contains("hr-first") {
            if lower.contains("disabled") || lower.contains("false") || lower.contains("off") || lower.contains("no") {
                out.hr_first_governor = Some(false);
            } else if lower.contains("enabled") || lower.contains("true") || lower.contains("active") || lower.contains("on") {
                out.hr_first_governor = Some(true);
            }
        }
        if out.primary_distance.is_none()
            && (lower.contains("primary distance") || lower.contains("primary event"))
        {
            if let Some((_, rhs)) = cleaned.split_once(':') {
                let v = rhs.trim();
                if !v.is_empty() {
                    out.primary_distance = Some(v.to_string());
                }
            }
        }
    }
    out
}

pub fn hydrate_profile_from_proposal(
    proposal: &CoachUpdateProposal,
    profile: &mut UserProfile,
    config: &mut AthleteConfig,
) -> bool {
    hydrate_profile_from_proposal_with_weight_guard(proposal, profile, config, false)
}

pub fn hydrate_profile_from_proposal_with_weight_guard(
    proposal: &CoachUpdateProposal,
    profile: &mut UserProfile,
    config: &mut AthleteConfig,
    has_authoritative_garmin_weight: bool,
) -> bool {
    let md_fallback = parse_profile_thresholds_from_markdown(&proposal.updated_section_markdown);
    let merged = match proposal.profile_thresholds.as_ref() {
        Some(t) => t.merged(&md_fallback),
        None => md_fallback,
    };
    if merged.is_empty() {
        return false;
    }

    if let Some(v) = merged.vt1_bpm.filter(|&v| v > 0) {
        profile.runner_config.vt1_bpm = Some(v);
        config.specs.vt1_ceiling_bpm = Some(v);
        if config.medical.easy_hr_ceiling_bpm.is_none() {
            config.medical.easy_hr_ceiling_bpm = Some(v);
        }
    }
    if let Some(v) = merged.vt2_bpm.filter(|&v| v > 0) {
        profile.runner_config.vt2_bpm = Some(v);
    }
    if let Some(v) = merged.vt1_pace_sec_per_km.filter(|&v| v > 0) {
        profile.runner_config.vt1_pace_sec_per_km = Some(v);
    }
    if let Some(v) = merged.vt2_pace_sec_per_km.filter(|&v| v > 0) {
        profile.runner_config.vt2_pace_sec_per_km = Some(v);
    }
    if let Some(v) = merged.lthr_bpm.filter(|&v| v > 0) {
        profile.runner_config.lthr_bpm = Some(v);
    }
    if let Some(v) = merged.max_hr_bpm.filter(|&v| v > 0) {
        profile.runner_config.max_hr_bpm = Some(v);
        config.specs.max_hr = Some(v);
    }
    if let Some(v) = merged.resting_hr_bpm.filter(|&v| v > 0) {
        profile.runner_config.resting_hr_bpm = Some(v);
        config.specs.resting_hr_baseline = Some(v);
    }
    if let Some(z2) = merged.z2_range_bpm {
        profile.runner_config.z2_range_bpm = Some(z2);
    }
    if let Some(z3) = merged.z3_range_bpm {
        profile.runner_config.z3_range_bpm = Some(z3);
    }
    if let Some(ref d) = merged.primary_distance {
        if !d.trim().is_empty() {
            profile.runner_config.primary_distance = Some(d.trim().to_string());
        }
    }
    if let Some(gov) = merged.hr_first_governor {
        profile.runner_config.hr_first_governor = gov;
        config.medical.enforce_easy_hr_ceiling = gov;
    }
    if let Some(pf) = merged.hard_pace_floor_sec_per_km.filter(|&v| v > 0) {
        profile.runner_config.pace_floor_sec_per_km = Some(pf);
        config.medical.enforce_pace_floor = true;
        config.medical.min_pace_sec_per_km = Some(pf);
    }
    if !has_authoritative_garmin_weight {
        if let Some(w) = merged.weight_kg.filter(|&w| w > 0.0) {
            config.specs.weight_kg = Some(w);
            config.specs.weight_baseline_confirmed = true;
        }
    }

    if let Some(ref div) = merged.hyrox_division {
        profile.hyrox_config.division = Some(div.clone());
    }
    if let Some(ref mode) = merged.hyrox_watch_logging_mode {
        profile.hyrox_config.watch_logging_mode = Some(mode.clone());
    }
    if let Some(tot) = merged.hyrox_target_total_time_sec {
        profile.hyrox_config.target_total_time_sec = Some(tot);
    }
    if let Some(fresh) = merged.hyrox_fresh_1km_pace_sec {
        profile.hyrox_config.fresh_1km_pace_sec = Some(fresh);
    }
    if let Some(comp) = merged.hyrox_compromised_1km_target_sec {
        profile.hyrox_config.compromised_1km_target_sec = Some(comp);
    }
    if let Some(ref weak) = merged.hyrox_weak_stations {
        if !weak.is_empty() {
            profile.hyrox_config.weak_stations = weak.clone();
        }
    }
    true
}

pub fn build_static_system_instruction(
    store: &SharedStore,
    config: &AthleteConfig,
    activities: &[ActivityRecord],
    user_profile: Option<&UserProfile>,
    first_use_state: Option<&FirstUseFlowState>,
) -> String {
    build_static_system_instruction_with_weights(
        store,
        config,
        activities,
        &[],
        user_profile,
        first_use_state,
        Utc::now().timestamp(),
    )
}

pub fn build_static_system_instruction_with_weights(
    store: &SharedStore,
    config: &AthleteConfig,
    activities: &[ActivityRecord],
    weights: &[WeightEntry],
    user_profile: Option<&UserProfile>,
    first_use_state: Option<&FirstUseFlowState>,
    now_epoch: i64,
) -> String {
    let profile_md = store.read_text("COACH_PROFILE.md").ok().flatten().unwrap_or_default();
    let plan_md = store.read_text("TRAINING_PLAN.md").ok().flatten().unwrap_or_default();
    let needs_setup = profile_md.trim().is_empty() || plan_md.trim().is_empty();

    let mut sections = vec![
        "You are a Living Markdown training-planning coach inside Splits Coach. Use metric units (km, min/km, bpm, kg). Always use `propose_coach_update` for Markdown changes, and CONSOLIDATE all sections for a given target file (`COACH_PROFILE.md` or `TRAINING_PLAN.md`) into ONE single `propose_coach_update` call per file rather than emitting separate proposals for individual sections. Use `get_recent_activities`, `get_activity_detail`, `get_wellness_history`, `get_training_load_and_records`, `get_readiness_and_plan`, `get_lifetime_and_yearly_history`, `fetch_1hz_window`, `append_training_ledger`, `update_post_run_context`, and `query_run_slice` for drill-downs. Inspect `garmin_notes` on activities when present. Weight Authority Rule: Garmin Connect weigh-in data is the authoritative weight source and always supersedes any older or placeholder weight in COACH_PROFILE.md or TRAINING_PLAN.md. Sweat Matrix Rule: when analyzing a run with `sweat_status == \"provisional_no_intake\"`, report the provisional sweat rate and ask if the user had any intra-run fluid intake, then call `update_post_run_context` once confirmed (`0` ml if none). Weight Fallback Rule: inspect `weight_source` — when set to `\"profile_baseline\"` or `\"latest_14d_weigh_in\"`, label derived sweat/weight metrics provisional. Hybrid Tempe Rule: when `temp_source == \"watch_internal\"` and temperature data exists, check the user's post-run message for any mention of a Garmin Tempe sensor and call `update_post_run_context(tempe_sensor_used: true)` -> `\"user_confirmed_tempe\"` if mentioned, or ask the user to confirm whether a Tempe sensor was worn before treating temperature spikes as external ambient heat rather than wrist skin warmth.".to_string(),
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
    let windows = scale_matcher::activity_windows(activities);
    if let Some(latest_weight) = scale_matcher::latest_baseline_weigh_in(weights, &windows) {
        let mut extras = Vec::new();
        if let Some(bf) = latest_weight.body_fat_pct {
            extras.push(format!("body fat {bf:.1}%"));
        }
        if let Some(mm) = latest_weight.muscle_mass_kg {
            extras.push(format!("muscle mass {mm:.1} kg"));
        }
        let extra = if extras.is_empty() {
            String::new()
        } else {
            format!(" · {}", extras.join(" · "))
        };
        let date_str = latest_weight.timestamp_iso.get(..10).unwrap_or("");
        let stale_tag = if scale_matcher::is_fresh_baseline(latest_weight, now_epoch) {
            ""
        } else {
            " (stale: >14d)"
        };
        sections.push(format!(
            "=== GARMIN CONNECT WEIGHT (AUTHORITATIVE) ===\nLatest non-dehydrated weigh-in: {:.1} kg on {date_str}{stale_tag} (source: {}{extra}). Supersedes any older weight in COACH_PROFILE.md or TRAINING_PLAN.md.",
            latest_weight.weight_kg, latest_weight.source
        ));
    }

    if let Some(fu) = first_use_state.filter(|s| s.stage.is_guided()) {
        let guardrail_note = "During initial profile setup, discuss and clarify `HR-First Governor` (`enforceEasyHRCeiling`), `Hard Pace Floor` (`minPaceSecPerKm`), easy HR ceiling, and minimum rest hours conversationally with the athlete. Never fabricate a goal time or target pace if `targetTimeDescription` is blank/unset.";
        match fu.stage {
            FirstUseStage::Intake | FirstUseStage::ProfileApproval => {
                sections.push(format!(
                    "GUIDED STAGE (Profile Synthesis): {guardrail_note} Synthesize ATHLETE CONFIG and any attached intake files (lab stress test PDF, Garmin stress .FIT, notes) into ONE consolidated `COACH_PROFILE.md` proposal via a single `propose_coach_update` call. Always populate `profile_thresholds` with every resolved threshold (vt1_bpm, vt2_bpm, vt1_pace_sec_per_km, vt2_pace_sec_per_km, lthr_bpm, max_hr_bpm, resting_hr_bpm, z2_range_bpm, z3_range_bpm, weight_kg, primary_distance, hr_first_governor, hard_pace_floor_sec_per_km). Do NOT propose `TRAINING_PLAN.md` yet — training block design waits until after Garmin history sync."
                ));
            }
            FirstUseStage::HistorySync => {
                sections.push(
                    "GUIDED STAGE (History Sync): Initial profile is approved. Full Garmin history download and TrainingHistoryAnalyzer audit run next before creating TRAINING_PLAN.md.".to_string(),
                );
            }
            FirstUseStage::PlanApproval => {
                let hist_block = fu
                    .history_summary_markdown
                    .as_deref()
                    .map(|m| format!("\n\nSynced Training History Summary:\n{m}"))
                    .unwrap_or_default();
                sections.push(format!(
                    "GUIDED STAGE (Plan Approval): Call `get_lifetime_and_yearly_history` if needed, confirm the athlete's preferred mesocycle block structure, and emit ONE consolidated `TRAINING_PLAN.md` proposal via a single `propose_coach_update` call (including structured `plan_json`). Never fabricate a target time or pace when none is configured.{hist_block}"
                ));
            }
            FirstUseStage::Active => {}
        }
    } else if needs_setup {
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
        if rep_clone.garmin_notes.is_none() {
            rep_clone.garmin_notes = activity.and_then(|a| a.normalized_notes().map(str::to_string));
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
        if let (Some(pre_off), Some(post_off)) = (ctx.pre_weigh_in_offset_min, ctx.post_weigh_in_offset_min) {
            let pre_s = if pre_off >= 0 { format!("+{pre_off} min") } else { format!("{pre_off} min") };
            let post_s = if post_off >= 0 { format!("+{post_off} min") } else { format!("{post_off} min") };
            lines.push(format!("- **Pairing Offsets**: Pre: {pre_s} · Post: {post_s}"));
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
        if let Some(notes) = activity.and_then(|a| a.normalized_notes()) {
            lines.push(format!("- **Garmin Connect Notes**: {notes}"));
        }
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
    } else if let Some(notes) = activity.and_then(|a| a.normalized_notes()) {
        blocks.push(format!("## Post-Run Context & Notes\n- **Garmin Connect Notes**: {notes}"));
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

pub fn without_dashes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '—' {
            while out.ends_with(' ') {
                out.pop();
            }
            while chars.peek() == Some(&' ') {
                chars.next();
            }
            if !out.is_empty() && chars.peek().is_some() {
                out.push_str(", ");
            }
        } else {
            out.push(c);
        }
    }
    out.replace("**", "")
}

fn format_hours_minutes_text(seconds: i32) -> String {
    let minutes = (seconds.max(0) + 30) / 60;
    if minutes >= 60 {
        format!("{} h {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

pub fn build_daily_briefing_facts(
    config: &AthleteConfig,
    activities: &[ActivityRecord],
    days: &[DayRecord],
    readiness: Option<&ReadinessState>,
    today_date: NaiveDate,
) -> String {
    build_daily_briefing_facts_with_weights(
        config,
        activities,
        days,
        &[],
        readiness,
        today_date,
        Utc::now().timestamp(),
    )
}

pub fn build_daily_briefing_facts_with_weights(
    config: &AthleteConfig,
    activities: &[ActivityRecord],
    days: &[DayRecord],
    weights: &[WeightEntry],
    readiness: Option<&ReadinessState>,
    today_date: NaiveDate,
    now_epoch: i64,
) -> String {
    let today_str = today_date.format("%Y-%m-%d").to_string();
    let yesterday_str = (today_date - Duration::days(1)).format("%Y-%m-%d").to_string();
    let two_weeks_ago_str = (today_date - Duration::days(14)).format("%Y-%m-%d").to_string();

    let today_day = days.iter().find(|d| d.date == today_str);
    let before: Vec<&DayRecord> = days
        .iter()
        .filter(|d| d.date >= two_weeks_ago_str && d.date < today_str)
        .collect();

    let avg_i32 = |vals: Vec<i32>| -> Option<i32> {
        if vals.is_empty() {
            None
        } else {
            Some((vals.iter().sum::<i32>() as f64 / vals.len() as f64).round() as i32)
        }
    };

    let mut lines = vec![format!(
        "Today is {}.",
        today_date.format("%A, %B %-d")
    )];
    let mut tired: Vec<String> = Vec::new();
    let mut fresh: Vec<String> = Vec::new();

    if let Some(d) = today_day {
        if let Some(sleep_sec) = d.sleep_seconds.or(d.sleep_duration_sec).filter(|&s| s > 0) {
            let mut line = format!("Last night: slept {}", format_hours_minutes_text(sleep_sec));
            if let Some(score) = d.sleep_score {
                line.push_str(&format!(", sleep score {score} out of 100"));
                if score < 60 {
                    tired.push("poor sleep score".to_string());
                } else if score >= 80 {
                    fresh.push("good sleep score".to_string());
                }
            }
            if let Some(usual) = avg_i32(
                before
                    .iter()
                    .filter_map(|x| x.sleep_seconds.or(x.sleep_duration_sec))
                    .collect(),
            ) {
                let diff_min = (sleep_sec - usual) / 60;
                if diff_min >= 30 {
                    line.push_str(&format!(", {diff_min} min more than usual"));
                    fresh.push("a long night's sleep".to_string());
                } else if diff_min <= -30 {
                    line.push_str(&format!(", {} min less than usual", -diff_min));
                    if diff_min <= -45 {
                        tired.push("short sleep".to_string());
                    }
                } else {
                    line.push_str(", about their usual");
                }
            }
            lines.push(format!("{line}."));
        } else {
            lines.push("Last night's sleep hasn't synced yet.".to_string());
        }

        if let Some(hrv) = d.hrv_last_night {
            let mut line = format!("Overnight HRV {hrv} ms");
            if let (Some(low), Some(high)) = (d.hrv_baseline_low, d.hrv_baseline_high) {
                if hrv < low {
                    line.push_str(&format!(", below their normal range of {low}–{high}"));
                    tired.push("HRV below normal".to_string());
                } else if hrv > high {
                    line.push_str(&format!(", above their normal range of {low}–{high}"));
                    fresh.push("HRV above normal".to_string());
                } else {
                    line.push_str(&format!(", inside their normal range of {low}–{high}"));
                    fresh.push("HRV in its normal range".to_string());
                }
            }
            if let Some(ref status) = d.hrv_status {
                let lower = status.to_lowercase();
                if lower.contains("low") || lower.contains("unbalanced") {
                    line.push_str(&format!("; the 7-day HRV status is {lower}"));
                    if !tired.iter().any(|t| t == "HRV below normal") {
                        tired.push(format!("HRV status {lower}"));
                    }
                }
            }
            lines.push(format!("{line}."));
        }

        let rhr_samples: Vec<(DateTime<Utc>, Option<i32>)> = days
            .iter()
            .filter_map(|x| {
                NaiveDate::parse_from_str(&x.date, "%Y-%m-%d")
                    .ok()
                    .and_then(|nd| nd.and_hms_opt(12, 0, 0))
                    .map(|ndt| (DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc), x.resting_hr))
            })
            .collect();
        let ref_dt = today_date
            .and_hms_opt(12, 0, 0)
            .map(|ndt| DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc))
            .unwrap_or_else(Utc::now);
        let resolved_rhr = config.resolved_resting_hr_baseline(&rhr_samples, ref_dt);
        let baseline_rhr = resolved_rhr
            .map(|r| r.bpm)
            .or_else(|| avg_i32(before.iter().filter_map(|x| x.resting_hr).collect()));
        if let (Some(rhr), Some(usual)) = (d.resting_hr, baseline_rhr) {
            let diff = rhr - usual;
            let desc = if diff >= 3 {
                tired.push("resting heart rate up".to_string());
                format!("{diff} above their usual {usual}")
            } else if diff <= -2 {
                fresh.push("resting heart rate normal".to_string());
                format!("{} below their usual {usual}", -diff)
            } else {
                fresh.push("resting heart rate normal".to_string());
                format!("about their usual {usual}")
            };
            let source_tag = resolved_rhr
                .map(|r| format!(" (rhr_baseline_source: {})", r.source))
                .unwrap_or_default();
            lines.push(format!("Resting heart rate {rhr} bpm, {desc}{source_tag}."));
        }

        if let Some(high) = d.body_battery_high {
            let usual = avg_i32(before.iter().filter_map(|x| x.body_battery_high).collect());
            let mut line = format!("Energy level (0 to 100) peaked at {high}");
            if let Some(now_bb) = d.body_battery_latest {
                line.push_str(&format!(" ({now_bb} now)"));
            }
            if let Some(u) = usual {
                if high >= u + 8 {
                    line.push_str(&format!(", higher than usual ({u})"));
                } else if high <= u - 8 {
                    line.push_str(&format!(", lower than usual ({u})"));
                } else {
                    line.push_str(", about usual");
                }
                if high <= u - 10 || high < 40 {
                    tired.push("low energy level".to_string());
                } else if high >= 75 {
                    fresh.push("energy level well charged".to_string());
                }
            } else if high >= 75 {
                fresh.push("energy level well charged".to_string());
            } else if high < 40 {
                tired.push("low energy level".to_string());
            }
            lines.push(format!("{line}."));
        }
    } else {
        lines.push("Last night's wellness data hasn't synced yet.".to_string());
    }

    let describe_act = |a: &ActivityRecord| -> String {
        let mut s = if a.name.trim().is_empty() {
            a.type_key.replace('_', " ")
        } else {
            a.name.trim().to_string()
        };
        if a.distance_m > 50.0 {
            s.push_str(&format!(" of {:.2} km", a.distance_m / 1000.0));
        }
        let dur = if a.moving_duration_sec > 0.0 {
            a.moving_duration_sec
        } else {
            a.duration_sec
        };
        s.push_str(&format!(" ({})", format_duration(dur)));
        s
    };

    let todays: Vec<String> = activities
        .iter()
        .filter(|a| a.start_iso.starts_with(&today_str))
        .map(describe_act)
        .collect();
    let yesterdays: Vec<String> = activities
        .iter()
        .filter(|a| a.start_iso.starts_with(&yesterday_str))
        .map(describe_act)
        .collect();
    if yesterdays.is_empty() {
        lines.push("Yesterday: no training.".to_string());
    } else {
        lines.push(format!("Yesterday: {}.", yesterdays.join(", ")));
    }
    if !todays.is_empty() {
        lines.push(format!("Already done today: {}.", todays.join(", ")));
    }

    let week_start_str = (today_date - Duration::days(7)).format("%Y-%m-%d").to_string();
    let base_start_str = (today_date - Duration::days(35)).format("%Y-%m-%d").to_string();
    let load_week: f64 = activities
        .iter()
        .filter(|a| {
            let d = a.start_iso.get(..10).unwrap_or("");
            d >= week_start_str.as_str() && d < today_str.as_str()
        })
        .filter_map(|a| a.training_load)
        .sum();
    let usual_load: f64 = activities
        .iter()
        .filter(|a| {
            let d = a.start_iso.get(..10).unwrap_or("");
            d >= base_start_str.as_str() && d < week_start_str.as_str()
        })
        .filter_map(|a| a.training_load)
        .sum::<f64>()
        / 4.0;
    if usual_load > 1.0 {
        let ratio = load_week / usual_load;
        let pct = ((ratio - 1.0).abs() * 100.0).round() as i32;
        if ratio >= 1.15 {
            lines.push(format!(
                "Training over the last 7 days: {pct}% more than a usual week (a big week)."
            ));
            if ratio >= 1.3 {
                tired.push("a much bigger week than usual".to_string());
            }
        } else if ratio <= 0.85 {
            lines.push(format!(
                "Training over the last 7 days: {pct}% less than a usual week (a light week)."
            ));
            fresh.push("a light week".to_string());
        } else {
            lines.push("Training over the last 7 days: about a usual week.".to_string());
        }
    }

    let last_night_in = today_day
        .map(|d| d.sleep_seconds.or(d.sleep_duration_sec).is_some() || d.hrv_last_night.is_some())
        .unwrap_or(false);
    let (verdict, plan_line) = if !last_night_in {
        (
            "unknown, last night's data hasn't synced from their watch yet",
            "No call on recovery yet; go by feel until the watch syncs.",
        )
    } else if tired.is_empty() {
        (
            "well recovered",
            "Cleared within your configured training envelope and heart rate ceiling.",
        )
    } else if tired.len() <= 2 {
        (
            "partly recovered",
            "Keep it easy today: stay within your easy aerobic ceiling, or rest if legs feel flat.",
        )
    } else {
        (
            "not well recovered",
            "Take a rest day today, or something very light like a gentle walk.",
        )
    };
    let mut verdict_line = format!("Recovery verdict: {verdict}.");
    if !tired.is_empty() {
        verdict_line.push_str(&format!(" Against: {}.", tired.join(", ")));
    }
    if !fresh.is_empty() {
        verdict_line.push_str(&format!(" For: {}.", fresh.join(", ")));
    }
    lines.push(verdict_line);
    lines.push(format!("Recommendation: {plan_line}"));

    if let Some(r) = readiness {
        let total_budget = r.gatekeeper.rolling_7d_km + r.gatekeeper.remaining_weekly_budget_km;
        lines.push(format!(
            "Authoritative readiness gatekeeper: decision={} (run_permitted={}), distance={:.1} km, duration={} min, HR ceiling={} bpm, shoe={}, phase={}, weekly envelope={:.1}/{:.1} km used ({:.1} km left, today max {:.1} km).",
            r.prescription_summary.decision,
            r.gatekeeper.run_permitted,
            r.prescription_summary.distance_km,
            r.prescription_summary.duration_mins,
            r.prescription_summary.hr_ceiling_bpm,
            r.prescription_summary.shoe_platform,
            r.plan_context.mesocycle_phase,
            r.gatekeeper.rolling_7d_km,
            total_budget,
            r.gatekeeper.remaining_weekly_budget_km,
            r.gatekeeper.effective_today_max_km
        ));
        if !r.gatekeeper.hard_blocks.is_empty() {
            lines.push(format!("Hard blocks: {}", r.gatekeeper.hard_blocks.join("; ")));
        }
        if !r.gatekeeper.caution_flags.is_empty() {
            lines.push(format!("Caution flags: {}", r.gatekeeper.caution_flags.join("; ")));
        }
    }

    let windows = scale_matcher::activity_windows(activities);
    if let Some(latest_weight) = scale_matcher::latest_baseline_weigh_in(weights, &windows) {
        let date_str = latest_weight.timestamp_iso.get(..10).unwrap_or("");
        let stale_tag = if scale_matcher::is_fresh_baseline(latest_weight, now_epoch) {
            ""
        } else {
            " (stale: >14d)"
        };
        lines.push(format!(
            "Garmin Connect weight: {:.1} kg on {date_str}{stale_tag} (authoritative; supersedes any older placeholder weight in COACH_PROFILE.md).",
            latest_weight.weight_kg
        ));
    }

    lines.join("\n")
}

pub async fn generate_daily_briefing(
    api_key: &str,
    store: &SharedStore,
    config: &AthleteConfig,
    activities: &[ActivityRecord],
    days: &[DayRecord],
    readiness: Option<&ReadinessState>,
    model_settings: &ModelSettings,
    today_date: NaiveDate,
) -> Result<DailyBriefingRecord, String> {
    generate_daily_briefing_with_weights(
        api_key,
        store,
        config,
        activities,
        days,
        &[],
        readiness,
        model_settings,
        today_date,
        Utc::now().timestamp(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn generate_daily_briefing_with_weights(
    api_key: &str,
    store: &SharedStore,
    config: &AthleteConfig,
    activities: &[ActivityRecord],
    days: &[DayRecord],
    weights: &[WeightEntry],
    readiness: Option<&ReadinessState>,
    model_settings: &ModelSettings,
    today_date: NaiveDate,
    now_epoch: i64,
) -> Result<DailyBriefingRecord, String> {
    let resolved = resolve_model(ModelScope::Background, model_settings);
    let today_str = today_date.format("%Y-%m-%d").to_string();
    let today_day = days.iter().find(|d| d.date == today_str);
    let had_sleep = today_day
        .and_then(|d| d.sleep_seconds.or(d.sleep_duration_sec))
        .map_or(false, |s| s > 0);

    let facts = build_daily_briefing_facts_with_weights(
        config, activities, days, weights, readiness, today_date, now_epoch,
    );
    let name = config.specs.display_name.trim();
    let greeting = if name.is_empty() {
        "You are a witty, upbeat running buddy and coach writing someone's morning briefing inside Splits Coach.".to_string()
    } else {
        format!("You are {name}'s witty, upbeat running buddy and coach writing their morning briefing inside Splits Coach. Address them warmly.")
    };

    let mut sys_parts = vec![
        format!(
            "{greeting}\nUse metric units (km, min/km, bpm, kg). Never override an authoritative gatekeeper hard block or running prohibition. Never invent numbers not present in the facts. Never use em dashes (—); use commas or full stops instead. Return valid JSON matching the schema: headline, body, action_cue."
        ),
        format!("=== ATHLETE CONFIG ===\n{}", config.prompt_summary(None)),
    ];
    let windows = scale_matcher::activity_windows(activities);
    if let Some(latest_weight) = scale_matcher::latest_baseline_weigh_in(weights, &windows) {
        let date_str = latest_weight.timestamp_iso.get(..10).unwrap_or("");
        let stale_tag = if scale_matcher::is_fresh_baseline(latest_weight, now_epoch) {
            ""
        } else {
            " (stale: >14d)"
        };
        sys_parts.push(format!(
            "=== GARMIN CONNECT WEIGHT (AUTHORITATIVE) ===\nLatest non-dehydrated weigh-in: {:.1} kg on {date_str}{stale_tag} (source: {}). Supersedes any older placeholder weight.",
            latest_weight.weight_kg, latest_weight.source
        ));
    }
    if let Ok(Some(profile_md)) = store.read_text("COACH_PROFILE.md") {
        let trimmed = profile_md.trim();
        if !trimmed.is_empty() {
            sys_parts.push(format!(
                "=== COACH_PROFILE.md ===\n{}",
                trimmed.chars().take(3000).collect::<String>()
            ));
        }
    }
    if let Ok(Some(plan_md)) = store.read_text("TRAINING_PLAN.md") {
        let trimmed = plan_md.trim();
        if !trimmed.is_empty() {
            sys_parts.push(format!(
                "=== TRAINING_PLAN.md ===\n{}",
                trimmed.chars().take(3000).collect::<String>()
            ));
        }
    }
    sys_parts.push(
        "Morning briefing format:\n- headline: one short punchy line (under 10 words, one relevant emoji) matching the recovery verdict.\n- body: 2-3 short paragraphs (1-2 sentences each, under 90 words total, plain text without markdown lists or bold).\n  1) Overnight recovery & biometrics against usual baseline with a light comparison.\n  2) Yesterday's training (or rest) and recent 7-day trend.\n  3) Today's clear call aligned with the gatekeeper decision.\n- action_cue: one crisp, concrete next step (respecting HR ceiling, distance cap, or rest rule)."
            .to_string(),
    );
    #[cfg(feature = "beta-coach")]
    sys_parts.push(
        "BETA COACH PERSONA DIRECTIVE: Keep every physiological metric and gatekeeper constraint 100% accurate while adding dry, deadpan wit."
            .to_string(),
    );

    let system_instruction = sys_parts.join("\n\n");
    let endpoint = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        resolved.model_id
    );
    let body = json!({
        "systemInstruction": { "parts": [{ "text": system_instruction }] },
        "contents": [{ "role": "user", "parts": [{ "text": facts }] }],
        "generationConfig": {
            "temperature": 0.4,
            "maxOutputTokens": 4096,
            "thinkingConfig": { "thinkingLevel": resolved.thinking_level.as_str() },
            "responseMimeType": "application/json",
            "responseSchema": {
                "type": "OBJECT",
                "properties": {
                    "headline": { "type": "STRING", "description": "Short headline under 10 words with one relevant emoji." },
                    "body": { "type": "STRING", "description": "2-3 concise paragraphs grounded strictly in the supplied telemetry." },
                    "action_cue": { "type": "STRING", "description": "One concrete next step respecting the athlete's guardrails." }
                },
                "required": ["headline", "body", "action_cue"]
            }
        }
    });

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(&endpoint)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if !resp.status().is_success() {
        let code = resp.status();
        if code.as_u16() == 429 {
            return Err("Gemini rate limit reached (HTTP 429). Please wait a moment before trying again.".to_string());
        }
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Gemini API error ({code}): {text}"));
    }

    let envelope: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to decode Gemini briefing response: {e}"))?;

    if let Some(usage) = envelope.get("usageMetadata") {
        let entry = CoachUsageEntry {
            timestamp: Utc::now().to_rfc3339(),
            profile: "DAILY_BRIEFING".to_string(),
            model: resolved.model_id.clone(),
            prompt_tokens: usage["promptTokenCount"].as_i64().unwrap_or(0) as i32,
            candidates_tokens: usage["candidatesTokenCount"].as_i64().unwrap_or(0) as i32,
            thoughts_tokens: usage["thoughtsTokenCount"].as_i64().unwrap_or(0) as i32,
            total_tokens: usage["totalTokenCount"].as_i64().unwrap_or(0) as i32,
            finish_reason: envelope["candidates"][0]["finishReason"]
                .as_str()
                .unwrap_or("STOP")
                .to_string(),
        };
        let _ = store.append_usage_log(&entry);
    }

    let parts = envelope["candidates"][0]["content"]["parts"]
        .as_array()
        .ok_or_else(|| "Gemini returned no briefing candidate parts.".to_string())?;
    let raw_text = parts
        .iter()
        .filter(|p| p["thought"].as_bool() != Some(true))
        .filter_map(|p| p["text"].as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
        .ok_or_else(|| "Gemini returned empty briefing text.".to_string())?;

    let mut clean = raw_text.trim();
    if let Some(rest) = clean.strip_prefix("```json") {
        clean = rest.trim();
    } else if let Some(rest) = clean.strip_prefix("```") {
        clean = rest.trim();
    }
    if let Some(rest) = clean.strip_suffix("```") {
        clean = rest.trim();
    }

    let parsed: Value = serde_json::from_str(clean)
        .map_err(|e| format!("Failed to parse structured daily briefing JSON: {e}"))?;

    let headline = without_dashes(
        parsed["headline"]
            .as_str()
            .unwrap_or("Today's readiness")
            .trim(),
    );
    let body = without_dashes(parsed["body"].as_str().unwrap_or("").trim());
    let action_cue = without_dashes(parsed["action_cue"].as_str().unwrap_or("").trim());

    Ok(DailyBriefingRecord {
        date: today_str,
        headline,
        body,
        action_cue,
        model_id: resolved.model_id,
        thinking_level: resolved.thinking_level.as_str().to_string(),
        had_sleep,
        generated_at: Utc::now().to_rfc3339(),
    })
}

fn mean_f64(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        None
    } else {
        Some(xs.iter().sum::<f64>() / xs.len() as f64)
    }
}

fn std_dev_f64(xs: &[f64]) -> Option<f64> {
    if xs.len() < 2 {
        return None;
    }
    let m = mean_f64(xs)?;
    let var = xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (xs.len() - 1) as f64;
    Some(var.sqrt())
}

fn median_f64(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let mid = s.len() / 2;
    if s.len() % 2 == 1 {
        Some(s[mid])
    } else {
        Some((s[mid - 1] + s[mid]) / 2.0)
    }
}

fn clock_hours_relative_to_wake_day(iso: &str, wake_day: NaiveDate) -> Option<f64> {
    let trimmed = iso.trim();
    let prefix: String = trimmed.chars().take(19).collect();
    let ndt = NaiveDateTime::parse_from_str(&prefix, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(&prefix, "%Y-%m-%d %H:%M:%S"))
        .ok()?;
    let prev_midnight = (wake_day - Duration::days(1)).and_hms_opt(0, 0, 0)?;
    let diff = ndt.signed_duration_since(prev_midnight).num_seconds() as f64 / 3600.0;
    Some(diff)
}

fn format_clock_hours(hours: f64) -> String {
    let mut mins = (hours * 60.0).round() as i32 % (24 * 60);
    if mins < 0 {
        mins += 24 * 60;
    }
    format!("{:02}:{:02}", mins / 60, mins % 60)
}

pub fn build_sleep_take_facts(days: &[DayRecord], today_date: NaiveDate) -> Option<String> {
    struct NightRow {
        date: NaiveDate,
        asleep_sec: f64,
        need_sec: Option<f64>,
        deep: f64,
        light: f64,
        rem: f64,
        bedtime_h: Option<f64>,
        midsleep_h: Option<f64>,
        is_free_day: bool,
    }

    let mut nights: Vec<NightRow> = days
        .iter()
        .filter_map(|d| {
            let nd = NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").ok()?;
            let asleep = d.sleep_seconds.or(d.sleep_duration_sec).filter(|&s| s > 0)? as f64;
            let need_min = d.sleep_need_minutes.or(d.sleep_baseline_minutes);
            let need_sec = need_min.filter(|&m| m > 0).map(|m| (m as f64) * 60.0);
            let start_h = d
                .sleep_start
                .as_deref()
                .and_then(|s| clock_hours_relative_to_wake_day(s, nd));
            let end_h = d
                .sleep_end
                .as_deref()
                .and_then(|s| clock_hours_relative_to_wake_day(s, nd));
            let (bedtime_h, midsleep_h) = match (start_h, end_h) {
                (Some(s), Some(e)) if e > s => {
                    let dur = e - s;
                    (Some(s - 24.0), Some((s + dur / 2.0) - 24.0))
                }
                _ => (None, None),
            };
            let wd = nd.weekday();
            let is_free_day = matches!(wd, chrono::Weekday::Sat | chrono::Weekday::Sun);
            Some(NightRow {
                date: nd,
                asleep_sec: asleep,
                need_sec,
                deep: d.sleep_deep_sec.unwrap_or(0).max(0) as f64,
                light: d.sleep_light_sec.unwrap_or(0).max(0) as f64,
                rem: d.sleep_rem_sec.unwrap_or(0).max(0) as f64,
                bedtime_h,
                midsleep_h,
                is_free_day,
            })
        })
        .collect();

    nights.sort_by_key(|n| n.date);
    let last = nights.last()?;
    let mut reasons: Vec<String> = Vec::new();
    let mut observations: Vec<String> = Vec::new();

    let hm = |s: f64| format_hours_minutes_text(s.round() as i32);
    if let Some(need) = last.need_sec {
        let diff = last.asleep_sec - need;
        if diff.abs() < 15.0 * 60.0 {
            reasons.push(format!(
                "Last night they slept {}, about what their watch says they needed.",
                hm(last.asleep_sec)
            ));
        } else if diff < 0.0 {
            reasons.push(format!(
                "Last night they slept {}, {} less than their watch says they needed.",
                hm(last.asleep_sec),
                hm(-diff)
            ));
        } else {
            reasons.push(format!(
                "Last night they slept {}, {} more than their watch says they needed.",
                hm(last.asleep_sec),
                hm(diff)
            ));
        }
    } else {
        reasons.push(format!("Last night they slept {}.", hm(last.asleep_sec)));
    }

    let debt_start = today_date - Duration::days(14);
    let balance_14: Vec<f64> = nights
        .iter()
        .filter(|n| n.date > debt_start)
        .filter_map(|n| n.need_sec.map(|need| n.asleep_sec - need))
        .collect();
    let debt_sec = if balance_14.len() >= 5 {
        let owed = (-balance_14.iter().sum::<f64>()).max(0.0);
        if owed < 30.0 * 60.0 {
            reasons.push("Over the last 14 nights there's no sleep debt to speak of.".to_string());
        } else {
            reasons.push(format!(
                "Over the last 14 nights they're {} short in total.",
                hm(owed)
            ));
        }
        Some(owed)
    } else {
        None
    };

    let month_ago = today_date - Duration::days(30);
    let recent: Vec<&NightRow> = nights.iter().filter(|n| n.date > month_ago).collect();
    let bedtimes: Vec<f64> = recent.iter().filter_map(|n| n.bedtime_h).collect();
    let mut bedtime_spread_min: Option<f64> = None;
    let mut usual_bedtime_h: Option<f64> = None;
    if bedtimes.len() >= 7 {
        if let (Some(med), Some(sd)) = (median_f64(&bedtimes), std_dev_f64(&bedtimes)) {
            let spread = sd * 60.0;
            bedtime_spread_min = Some(spread);
            usual_bedtime_h = Some(med);
            let judgement = if spread < 30.0 {
                "very regular"
            } else if spread < 60.0 {
                "fairly regular"
            } else {
                "irregular"
            };
            observations.push(format!(
                "Their bedtime is usually around {} and is {} from night to night.",
                format_clock_hours(med),
                judgement
            ));
        }
    }

    let last_60: Vec<&NightRow> = nights.iter().rev().take(60).collect();
    let free_mids: Vec<f64> = last_60
        .iter()
        .filter(|n| n.is_free_day)
        .filter_map(|n| n.midsleep_h)
        .collect();
    let work_mids: Vec<f64> = last_60
        .iter()
        .filter(|n| !n.is_free_day)
        .filter_map(|n| n.midsleep_h)
        .collect();
    let mut social_jetlag_h: Option<f64> = None;
    if free_mids.len() >= 3 && work_mids.len() >= 5 {
        if let (Some(msf), Some(msw)) = (mean_f64(&free_mids), mean_f64(&work_mids)) {
            let jetlag = msf - msw;
            social_jetlag_h = Some(jetlag);
            if jetlag.abs() >= 0.75 {
                observations.push(format!(
                    "On weekends they sleep {} {} than on weekdays (social jetlag).",
                    hm(jetlag.abs() * 3600.0),
                    if jetlag > 0.0 { "later" } else { "earlier" }
                ));
            }
        }
    }

    let with_stages: Vec<&&NightRow> = recent
        .iter()
        .filter(|n| n.deep + n.light + n.rem > 0.0)
        .collect();
    let mut low_stages: Vec<&str> = Vec::new();
    if !with_stages.is_empty() {
        let deep_share = mean_f64(
            &with_stages
                .iter()
                .map(|n| n.deep / n.asleep_sec * 100.0)
                .collect::<Vec<_>>(),
        )
        .unwrap_or(0.0);
        let rem_share = mean_f64(
            &with_stages
                .iter()
                .map(|n| n.rem / n.asleep_sec * 100.0)
                .collect::<Vec<_>>(),
        )
        .unwrap_or(0.0);
        if deep_share < 16.0 {
            low_stages.push("Deep");
        }
        if rem_share < 21.0 {
            low_stages.push("REM");
        }
        if !low_stages.is_empty() {
            let joined = low_stages
                .iter()
                .map(|s| if *s == "REM" { "REM" } else { "deep" })
                .collect::<Vec<_>>()
                .join(" and ");
            observations.push(format!(
                "{} sleep {} below the share of the night considered optimal.",
                if joined.starts_with("REM") { "REM" } else { "Deep" },
                if low_stages.len() > 1 { "are" } else { "is" }
            ));
        }
    }

    let verdict = match debt_sec.unwrap_or(0.0) {
        d if d < 3600.0 => {
            if last
                .need_sec
                .map_or(true, |need| last.asleep_sec >= need - 30.0 * 60.0)
            {
                "Well rested"
            } else {
                "Short night, solid fortnight"
            }
        }
        d if d < 3.0 * 3600.0 => "Running a little short",
        _ => "Short on sleep",
    };

    let low_stage = if low_stages.contains(&"REM") {
        Some("REM")
    } else {
        low_stages.first().copied()
    };
    let advice = if debt_sec.unwrap_or(0.0) >= 2.0 * 3600.0 {
        "Pay some of it back: go to bed 30 to 45 minutes earlier for the next few nights.".to_string()
    } else if bedtime_spread_min.unwrap_or(0.0) >= 60.0 {
        format!(
            "Try to fall asleep within half an hour of {} every night, weekends too.",
            format_clock_hours(usual_bedtime_h.unwrap_or(-1.0))
        )
    } else if social_jetlag_h.unwrap_or(0.0) >= 1.0 {
        "On weekends, get up within an hour of the weekday time so Monday feels easier.".to_string()
    } else if low_stage == Some("REM") {
        "Most REM sleep comes in the last hours of the night, so protect the end of the night: a steady wake time and enough time in bed.".to_string()
    } else if low_stage == Some("Deep") {
        "Deep sleep comes early in the night; a calm last hour before bed and a regular bedtime help it most.".to_string()
    } else {
        "Keep doing what you're doing.".to_string()
    };

    let mut lines = vec![
        "Topic: their sleep, measured by the watch.".to_string(),
        format!("Verdict: {verdict}."),
    ];
    lines.extend(reasons);
    if !observations.is_empty() {
        lines.push("Observations:".to_string());
        lines.extend(observations);
    }
    lines.push(format!("Advice: {advice}"));
    Some(lines.join("\n"))
}

pub fn build_hrv_take_facts(
    days: &[DayRecord],
    activities: &[ActivityRecord],
    today_date: NaiveDate,
) -> Option<String> {
    struct Morning {
        date: NaiveDate,
        ms: f64,
        ln_ms: f64,
        baseline_low: Option<i32>,
        baseline_high: Option<i32>,
    }

    let mut mornings: Vec<Morning> = days
        .iter()
        .filter_map(|d| {
            let nd = NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").ok()?;
            let ms = d.hrv_last_night.filter(|&v| v > 0)? as f64;
            Some(Morning {
                date: nd,
                ms,
                ln_ms: ms.ln(),
                baseline_low: d.hrv_baseline_low,
                baseline_high: d.hrv_baseline_high,
            })
        })
        .collect();
    mornings.sort_by_key(|m| m.date);
    let last = mornings.last()?;

    struct SeriesPt {
        date: NaiveDate,
        mean_ln: f64,
        cv: f64,
        low_ln: Option<f64>,
        high_ln: Option<f64>,
        usual_cv: Option<f64>,
    }

    let mut series: Vec<SeriesPt> = Vec::new();
    for (i, m) in mornings.iter().enumerate() {
        let week_start = m.date - Duration::days(6);
        let week_lns: Vec<f64> = mornings[..=i]
            .iter()
            .rev()
            .take_while(|x| x.date >= week_start)
            .map(|x| x.ln_ms)
            .collect();
        if week_lns.len() < 4 {
            continue;
        }
        let Some(w_mean) = mean_f64(&week_lns) else { continue };
        let Some(w_sd) = std_dev_f64(&week_lns) else { continue };

        let base_start = m.date - Duration::days(60);
        let base_lns: Vec<f64> = mornings[..i]
            .iter()
            .filter(|x| x.date >= base_start)
            .map(|x| x.ln_ms)
            .collect();
        let (low_ln, high_ln) = if base_lns.len() >= 20 {
            if let (Some(bm), Some(bsd)) = (mean_f64(&base_lns), std_dev_f64(&base_lns)) {
                (Some(bm - 0.5 * bsd), Some(bm + 0.5 * bsd))
            } else {
                (None, None)
            }
        } else if let (Some(lo), Some(hi)) = (
            m.baseline_low.filter(|&v| v > 0),
            m.baseline_high.filter(|&v| v > 0),
        ) {
            (Some((lo as f64).ln()), Some((hi as f64).ln()))
        } else {
            (None, None)
        };

        let earlier_cvs: Vec<f64> = series
            .iter()
            .filter(|p| p.date >= base_start)
            .map(|p| p.cv)
            .collect();
        let usual_cv = if earlier_cvs.len() >= 10 {
            mean_f64(&earlier_cvs)
        } else {
            None
        };
        series.push(SeriesPt {
            date: m.date,
            mean_ln: w_mean,
            cv: if w_mean > 0.0 { w_sd / w_mean * 100.0 } else { 0.0 },
            low_ln,
            high_ln,
            usual_cv,
        });
    }

    let latest = series.last();
    let mut reasons = vec![format!("Last night's HRV was {} ms.", last.ms.round() as i32)];
    let mut observations: Vec<String> = Vec::new();

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum HrvLevel {
        Below,
        Within,
        Above,
    }

    let level = latest.and_then(|pt| match (pt.low_ln, pt.high_ln) {
        (Some(lo), Some(hi)) => {
            if pt.mean_ln < lo {
                Some(HrvLevel::Below)
            } else if pt.mean_ln > hi {
                Some(HrvLevel::Above)
            } else {
                Some(HrvLevel::Within)
            }
        }
        _ => None,
    });

    match level {
        Some(HrvLevel::Below) => reasons.push("The 7-day average is below their normal range.".to_string()),
        Some(HrvLevel::Above) => reasons.push("The 7-day average is above their normal range.".to_string()),
        Some(HrvLevel::Within) => reasons.push("The 7-day average is inside their normal range.".to_string()),
        None => {}
    }

    if let Some(pt) = latest {
        let week_ago = pt.date - Duration::days(7);
        if let Some(earlier) = series.iter().rev().find(|p| p.date <= week_ago) {
            let change = ((pt.mean_ln - earlier.mean_ln).exp() - 1.0) * 100.0;
            if change.abs() < 3.0 {
                reasons.push("It's been steady over the past week.".to_string());
            } else {
                reasons.push(format!(
                    "It has {} {}% over the past week.",
                    if change > 0.0 { "risen" } else { "fallen" },
                    change.abs().round() as i32
                ));
            }
        }
    }

    let jumpy = latest
        .and_then(|pt| pt.usual_cv.map(|u| pt.cv > u * 1.5))
        .unwrap_or(false);
    if jumpy {
        reasons.push("It's swinging more than usual from one morning to the next.".to_string());
    }

    let mut rhr_vals: Vec<i32> = days
        .iter()
        .filter_map(|d| NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").ok().zip(d.resting_hr))
        .filter(|&(_, r)| r > 0)
        .map(|(nd, r)| (nd, r))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|(_, r)| r)
        .collect();
    if rhr_vals.len() >= 24 {
        let rhr_now: Vec<f64> = rhr_vals.drain(..7).map(|v| v as f64).collect();
        let rhr_before: Vec<f64> = rhr_vals.into_iter().take(60).map(|v| v as f64).collect();
        if let (Some(now_m), Some(prev_m)) = (mean_f64(&rhr_now), mean_f64(&rhr_before)) {
            let diff = now_m - prev_m;
            if diff.abs() >= 2.0 {
                reasons.push(format!(
                    "Resting heart rate is {} bpm {} usual.",
                    diff.abs().round() as i32,
                    if diff > 0.0 { "above" } else { "below" }
                ));
            }
        }
    }

    let week_cutoff = (today_date - Duration::days(6)).format("%Y-%m-%d").to_string();
    let month_cutoff = (today_date - Duration::days(27)).format("%Y-%m-%d").to_string();
    let acute: f64 = activities
        .iter()
        .filter(|a| a.start_iso.get(..10).unwrap_or("") >= week_cutoff.as_str())
        .filter_map(|a| a.training_load)
        .sum();
    let chronic: f64 = activities
        .iter()
        .filter(|a| a.start_iso.get(..10).unwrap_or("") >= month_cutoff.as_str())
        .filter_map(|a| a.training_load)
        .sum::<f64>()
        / 4.0;
    if chronic > 0.0 {
        let ratio = acute / chronic;
        if ratio > 1.3 {
            observations.push(format!(
                "This week's training load is {ratio:.1} times the usual week, a big step up."
            ));
        } else if ratio < 0.7 {
            observations.push("This week's training has been lighter than usual.".to_string());
        }
    }

    let resting_up = reasons.iter().any(|r| r.contains("Resting heart rate is") && r.contains("above"));
    let (verdict, advice) = match level {
        Some(HrvLevel::Below) if jumpy || resting_up => (
            "Under strain",
            "Keep the next two or three days easy and give sleep priority, until the average is back in range.",
        ),
        Some(HrvLevel::Below) => (
            "A little low",
            "Go easy today and see where it is tomorrow; one hard session can wait.",
        ),
        Some(HrvLevel::Within) if jumpy => (
            "In range, but jumpy",
            "Train as planned but keep the easy days easy, and watch how it settles.",
        ),
        Some(HrvLevel::Within) => (
            "In balance",
            "The body is handling the training well, so carry on as planned.",
        ),
        Some(HrvLevel::Above) => (
            "Above normal",
            "A good window for a hard session, if the legs agree.",
        ),
        None => (
            "Still learning your normal",
            "Keep wearing the watch at night; after about 60 days there's a normal range to compare with.",
        ),
    };

    let mut lines = vec![
        "Topic: heart rate variability (HRV), measured overnight by the watch. Higher than your normal usually means well recovered.".to_string(),
        format!("Verdict: {verdict}."),
    ];
    lines.extend(reasons);
    if !observations.is_empty() {
        lines.push("Observations:".to_string());
        lines.extend(observations);
    }
    lines.push(format!("Advice: {advice}"));
    Some(lines.join("\n"))
}

pub async fn generate_health_take(
    scope: &str,
    api_key: &str,
    store: &SharedStore,
    config: &AthleteConfig,
    activities: &[ActivityRecord],
    days: &[DayRecord],
    model_settings: &ModelSettings,
    night_date: &str,
    today_date: NaiveDate,
) -> Result<DailyBriefingRecord, String> {
    let resolved = resolve_model(ModelScope::Background, model_settings);
    let facts = match scope {
        "sleep_take" => build_sleep_take_facts(days, today_date)
            .ok_or_else(|| "No sleep data available yet to summarize.".to_string())?,
        "hrv_take" => build_hrv_take_facts(days, activities, today_date)
            .ok_or_else(|| "No HRV data available yet to summarize.".to_string())?,
        other => return Err(format!("Unsupported health take scope: {other}")),
    };

    let name = config.specs.display_name.trim();
    let who = if name.is_empty() { "someone" } else { name };
    #[allow(unused_mut)]
    let mut sys_parts = vec![format!(
        "You are a friendly, sharp coach explaining {who}'s watch data to them like a smart friend would.\nReturn valid JSON matching the schema: headline, body, action_cue.\n- headline: one short, playful line (under 8 words, one relevant emoji) that puts the verdict in your own words.\n- body: 2 short paragraphs (1-2 sentences each, under 85 words total, plain text without markdown lists or bold) covering where things stand and the pattern behind it, using only numbers present in the facts.\n- action_cue: the advice, concisely worded.\nNever invent numbers or medical diagnoses. Never use em dashes (—); use commas or full stops instead."
    )];
    #[cfg(feature = "beta-coach")]
    sys_parts.push(
        "BETA COACH PERSONA DIRECTIVE: Keep every physiological metric 100% accurate while adding dry, deadpan wit."
            .to_string(),
    );

    let endpoint = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        resolved.model_id
    );
    let body = json!({
        "systemInstruction": { "parts": [{ "text": sys_parts.join("\n\n") }] },
        "contents": [{ "role": "user", "parts": [{ "text": facts }] }],
        "generationConfig": {
            "temperature": 0.4,
            "maxOutputTokens": 2048,
            "thinkingConfig": { "thinkingLevel": resolved.thinking_level.as_str() },
            "responseMimeType": "application/json",
            "responseSchema": {
                "type": "OBJECT",
                "properties": {
                    "headline": { "type": "STRING" },
                    "body": { "type": "STRING" },
                    "action_cue": { "type": "STRING" }
                },
                "required": ["headline", "body", "action_cue"]
            }
        }
    });

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(&endpoint)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if !resp.status().is_success() {
        let code = resp.status();
        if code.as_u16() == 429 {
            return Err("Gemini rate limit reached (HTTP 429). Please wait a moment before trying again.".to_string());
        }
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Gemini API error ({code}): {text}"));
    }

    let envelope: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to decode Gemini response: {e}"))?;

    if let Some(usage) = envelope.get("usageMetadata") {
        let entry = CoachUsageEntry {
            timestamp: Utc::now().to_rfc3339(),
            profile: scope.to_uppercase(),
            model: resolved.model_id.clone(),
            prompt_tokens: usage["promptTokenCount"].as_i64().unwrap_or(0) as i32,
            candidates_tokens: usage["candidatesTokenCount"].as_i64().unwrap_or(0) as i32,
            thoughts_tokens: usage["thoughtsTokenCount"].as_i64().unwrap_or(0) as i32,
            total_tokens: usage["totalTokenCount"].as_i64().unwrap_or(0) as i32,
            finish_reason: envelope["candidates"][0]["finishReason"]
                .as_str()
                .unwrap_or("STOP")
                .to_string(),
        };
        let _ = store.append_usage_log(&entry);
    }

    let parts = envelope["candidates"][0]["content"]["parts"]
        .as_array()
        .ok_or_else(|| "Gemini returned no candidate parts.".to_string())?;
    let raw_text = parts
        .iter()
        .filter(|p| p["thought"].as_bool() != Some(true))
        .filter_map(|p| p["text"].as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
        .ok_or_else(|| "Gemini returned empty text.".to_string())?;

    let mut clean = raw_text.trim();
    if let Some(rest) = clean.strip_prefix("```json") {
        clean = rest.trim();
    } else if let Some(rest) = clean.strip_prefix("```") {
        clean = rest.trim();
    }
    if let Some(rest) = clean.strip_suffix("```") {
        clean = rest.trim();
    }

    let parsed: Value = serde_json::from_str(clean)
        .map_err(|e| format!("Failed to parse structured health take JSON: {e}"))?;

    let default_head = if scope == "sleep_take" { "Your sleep" } else { "Your HRV" };
    let headline = without_dashes(parsed["headline"].as_str().unwrap_or(default_head).trim());
    let body_text = without_dashes(parsed["body"].as_str().unwrap_or("").trim());
    let action_cue = without_dashes(parsed["action_cue"].as_str().unwrap_or("").trim());

    Ok(DailyBriefingRecord {
        date: night_date.to_string(),
        headline,
        body: body_text,
        action_cue,
        model_id: resolved.model_id,
        thinking_level: resolved.thinking_level.as_str().to_string(),
        had_sleep: scope == "sleep_take",
        generated_at: Utc::now().to_rfc3339(),
    })
}
