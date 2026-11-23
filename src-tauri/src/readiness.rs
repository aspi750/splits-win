use crate::models::*;
use crate::storage::{SharedStore, StorageError};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::collections::HashSet;
use std::path::Path;

pub struct ReadinessInput<'a> {
    pub now: DateTime<Utc>,
    pub athlete_config: &'a AthleteConfig,
    pub activities: &'a [ActivityRecord],
    pub days: &'a [DayRecord],
    pub plan: Option<&'a PlanFile>,
    pub gear_inventory: Option<&'a GearInventoryFile>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadinessOutput {
    pub readiness_state: ReadinessState,
    pub authoritative_target: AuthoritativeTarget,
}

impl ReadinessOutput {
    pub fn persist(&self, shared_dir: &Path) -> Result<(), StorageError> {
        SharedStore::ensure_subdirectories(shared_dir)?;
        let readiness_bytes = serde_json::to_vec_pretty(&self.readiness_state)?;
        SharedStore::atomic_write_bytes(&shared_dir.join("readiness_state.json"), &readiness_bytes)?;

        let target_bytes = serde_json::to_vec_pretty(&self.authoritative_target)?;
        SharedStore::atomic_write_bytes(&shared_dir.join("today_adapted_target.json"), &target_bytes)?;
        Ok(())
    }
}

pub struct ReadinessEngine;

impl ReadinessEngine {
    /// Deterministic 1:1 port of `Splits/Store/ReadinessEngine.swift` (`CONTRACT.md §3`).
    pub fn evaluate(input: ReadinessInput<'_>) -> ReadinessOutput {
        let now = input.now;
        let today_str = now.format("%Y-%m-%d").to_string();
        let day_of_week_str = now.format("%A").to_string();
        let updated_at_str = now.to_rfc3339();

        // 1. Running activities & 7-day rolling load
        let mut prior_runs: Vec<&ActivityRecord> = input
            .activities
            .iter()
            .filter(|a| is_running_type(&a.type_key) && parse_iso_utc(&a.start_iso).map_or(false, |dt| dt <= now))
            .collect();
        prior_runs.sort_by(|a, b| b.start_iso.cmp(&a.start_iso));

        let hours_since_last_run: Option<f64> = prior_runs.first().and_then(|latest| {
            parse_iso_utc(&latest.start_iso).map(|dt| {
                let hrs = (now - dt).num_seconds() as f64 / 3600.0;
                round2(hrs.max(0.0))
            })
        });

        let goal_type = input.athlete_config.objective.goal_type;
        let is_hybrid = goal_type == GoalType::HybridHyrox;
        let is_reentry_or_c25k = goal_type.is_reentry_or_starter();
        let mut high_strain_sessions: Vec<&ActivityRecord> = input
            .activities
            .iter()
            .filter(|a| is_high_strain_session(a) && parse_iso_utc(&a.start_iso).map_or(false, |dt| dt <= now))
            .collect();
        high_strain_sessions.sort_by(|a, b| b.start_iso.cmp(&a.start_iso));

        let hours_since_last_high_strain: Option<f64> = high_strain_sessions.first().and_then(|latest| {
            parse_iso_utc(&latest.start_iso).map(|dt| {
                let hrs = (now - dt).num_seconds() as f64 / 3600.0;
                round2(hrs.max(0.0))
            })
        });

        let seven_days_ago = now - Duration::days(7);
        let hybrid_sessions_7d_count = high_strain_sessions
            .iter()
            .filter(|a| {
                !is_running_type(&a.type_key)
                    && parse_iso_utc(&a.start_iso).map_or(false, |dt| dt >= seven_days_ago)
            })
            .count();
        let runs_7d: Vec<&ActivityRecord> = prior_runs
            .iter()
            .copied()
            .filter(|a| parse_iso_utc(&a.start_iso).map_or(false, |dt| dt >= seven_days_ago))
            .collect();

        let rolling_7d_km = round2(runs_7d.iter().map(|a| a.distance_km()).sum());
        let rolling_7d_runs = runs_7d.len() as i32;

        let weekly_volume_cap_km = resolve_weekly_volume_cap_km(input.plan, input.athlete_config);
        let remaining_7d_budget_km = round2((weekly_volume_cap_km - rolling_7d_km).max(0.0));
        let max_runs_7d = input.plan.map_or(4, |p| (p.sessions.len() as i32).max(3));

        // 2. Today's recovery telemetry & 30d RHR baseline
        let today_day = input.days.iter().find(|d| d.date == today_str);
        let rhr_samples: Vec<(DateTime<Utc>, Option<i32>)> = input
            .days
            .iter()
            .filter_map(|d| {
                NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")
                    .ok()
                    .and_then(|nd| nd.and_hms_opt(12, 0, 0))
                    .map(|ndt| (DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc), d.resting_hr))
            })
            .collect();
        let baseline_rhr = input.athlete_config.effective_resting_hr_baseline(&rhr_samples, now);

        // 3. Hard Blocks (`CONTRACT.md §3.2`)
        let mut hard_blocks = Vec::new();
        let default_spacing = if is_reentry_or_c25k { 48 } else { 36 };
        let min_spacing_hours = input.athlete_config.medical.min_rest_hours_between_runs.unwrap_or(default_spacing) as f64;
        let spacing_ref_hours = if is_hybrid { hours_since_last_high_strain } else { hours_since_last_run };
        let spacing_label = if is_hybrid { "high-strain session" } else { "run" };
        if let Some(hrs) = spacing_ref_hours {
            if hrs < min_spacing_hours {
                hard_blocks.push(format!(
                    "STRUCTURAL_SPACING_VIOLATION: {hrs:.1}h since last {spacing_label} (min required: {}h).",
                    min_spacing_hours as i32
                ));
            }
        }

        if rolling_7d_runs >= max_runs_7d {
            hard_blocks.push(format!(
                "FREQUENCY_CAP_REACHED: {rolling_7d_runs} runs already completed in rolling 7 days (cap: {max_runs_7d})."
            ));
        }

        if remaining_7d_budget_km < 2.0 {
            hard_blocks.push(format!(
                "VOLUME_CAP_REACHED: Rolling 7-day volume is {rolling_7d_km:.1} km against {weekly_volume_cap_km:.1} km cap."
            ));
        }

        if let Some(score) = today_day.and_then(|d| d.sleep_score) {
            if score < 55 {
                hard_blocks.push(format!(
                    "CRITICAL_SLEEP_DEFICIT: Sleep score is {score} (minimum threshold: 55)."
                ));
            }
        }

        if let Some(sleep_sec) = today_day.and_then(|d| d.sleep_seconds) {
            let sleep_hours = round2(sleep_sec as f64 / 3600.0);
            if sleep_hours < 5.5 {
                hard_blocks.push(format!(
                    "CRITICAL_SLEEP_DURATION: Total sleep is {sleep_hours:.1}h (minimum threshold: 5.5h)."
                ));
            }
        }

        if let (Some(rhr), Some(base)) = (today_day.and_then(|d| d.resting_hr), baseline_rhr) {
            let delta = rhr - base;
            if delta >= 5 {
                hard_blocks.push(format!(
                    "ELEVATED_RHR: Resting HR is {rhr} bpm (+{delta} bpm over {base} bpm baseline)."
                ));
            }
        }

        if let Some(plan) = input.plan {
            if plan.envelope.max_weekly_km > 0.0 {
                let plan_remaining = round2((plan.envelope.max_weekly_km - rolling_7d_km).max(0.0));
                if plan_remaining < 2.0 && !hard_blocks.iter().any(|b| b.starts_with("VOLUME_CAP_REACHED")) {
                    hard_blocks.push(format!(
                        "WEEKLY_ENVELOPE_EXHAUSTED: Rolling 7d volume is {rolling_7d_km:.1} km against weekly envelope {:.1} km.",
                        plan.envelope.max_weekly_km
                    ));
                }
            }
        }

        // 4. Caution Flags (`CONTRACT.md §3.3`)
        let mut caution_flags = Vec::new();
        if let Some(status) = today_day.and_then(|d| d.hrv_status.as_deref()) {
            let upper = status.to_uppercase();
            if upper == "UNBALANCED" || upper == "LOW" || upper == "POOR" {
                let hrv_str = today_day
                    .and_then(|d| d.hrv_last_night)
                    .map(|v| format!("{v} ms"))
                    .unwrap_or_else(|| "N/A".to_string());
                caution_flags.push(format!(
                    "AUTONOMIC_SUPPRESSION: Overnight HRV status is {upper} ({hrv_str})."
                ));
            }
        }

        let cadence_breaches = runs_7d
            .iter()
            .filter(|a| a.cadence_spm.map_or(false, |c| c > 0.0 && c < 170.0))
            .count();
        if cadence_breaches > 0 {
            caution_flags.push(format!(
                "CADENCE_DRIFT_WARNING: {cadence_breaches} recent run(s) averaged < 170 spm."
            ));
        }

        let missing_morning = match today_day {
            None => true,
            Some(d) => d.sleep_seconds.is_none() && d.hrv_last_night.is_none() && d.resting_hr.is_none(),
        };
        if missing_morning {
            caution_flags.push(format!(
                "MISSING_MORNING_TELEMETRY: Sleep, HRV, and resting HR for {today_str} have not synced yet."
            ));
        }

        let run_permitted = hard_blocks.is_empty();

        // 5. Shoe Eligibility & Planned Session
        let (eligible_shoes, gated_shoes, blocked_shoes) = resolve_shoe_eligibility(input.gear_inventory, &prior_runs);
        let today_session = input.plan.and_then(|p| p.sessions.iter().find(|s| s.date == today_str));
        let default_single_cap_km = if goal_type == GoalType::CouchTo5k {
            5.0
        } else if is_reentry_or_c25k {
            6.0
        } else {
            10.0
        };
        let single_run_cap_km = input.plan.map_or(default_single_cap_km, |p| p.envelope.max_single_run_km);
        let effective_today_max_km = round2(single_run_cap_km.min(remaining_7d_budget_km));

        // 6. Deterministic Prescription & Guardrail Clamping (`CONTRACT.md §3.4`)
        let (status_badge, decision, prescribed_title, distance_km, duration_mins, shoe_platform, hr_ceiling_bpm, cadence_target_spm, execution_structure) =
            if !run_permitted {
                (
                    "🔴 RED - REST & DECOMPRESSION",
                    "REST_DECOMPRESSION",
                    "Rest from running".to_string(),
                    0.0,
                    0,
                    "N/A - Rest".to_string(),
                    105,
                    "N/A",
                    "No running session prescribed due to active gatekeeper hard block(s).".to_string(),
                )
            } else if !caution_flags.is_empty() {
                let planned_dist = today_session.and_then(|s| s.target.distance_km.first().copied()).unwrap_or(5.0);
                let dist = round2(planned_dist.min(remaining_7d_budget_km));
                let configured_easy = input
                    .athlete_config
                    .medical
                    .easy_hr_ceiling_bpm
                    .or(input.athlete_config.specs.vt1_ceiling_bpm)
                    .unwrap_or(138);
                let hr_cap = today_session.map_or(145, |s| s.hr_cap_bpm).min(configured_easy);
                let dur = today_session
                    .and_then(|s| s.target.duration_min.first().copied())
                    .map(|v| v.round() as i32)
                    .unwrap_or_else(|| ((dist * 6.5).round() as i32).max(20));
                let shoe = eligible_shoes.first().cloned().unwrap_or_else(|| "Daily Trainer".to_string());
                (
                    "🟡 AMBER - ADAPTED / CAUTION",
                    "ADAPTED_RUN",
                    "Adapted Easy Aerobic".to_string(),
                    dist,
                    dur,
                    shoe,
                    hr_cap,
                    "172-178 spm",
                    format!("{dist:.1} km easy aerobic within HR <= {hr_cap} bpm."),
                )
            } else {
                let fallback_dist = if is_reentry_or_c25k { default_single_cap_km } else { 6.0 };
                let planned_dist = today_session
                    .and_then(|s| s.target.distance_km.first().copied())
                    .unwrap_or_else(|| fallback_dist.min(remaining_7d_budget_km));
                let dist = round2(planned_dist.min(remaining_7d_budget_km));
                let hr_cap = today_session
                    .map(|s| s.hr_cap_bpm)
                    .or(input.athlete_config.specs.vt1_ceiling_bpm)
                    .or(input.athlete_config.medical.easy_hr_ceiling_bpm)
                    .unwrap_or(145);
                let pace_min_per_km = input.athlete_config.medical.min_pace_sec_per_km.unwrap_or(360) as f64 / 60.0;
                let dur = today_session
                    .and_then(|s| s.target.duration_min.first().copied())
                    .map(|v| v.round() as i32)
                    .unwrap_or_else(|| ((dist * pace_min_per_km).round() as i32).max(20));
                let shoe = eligible_shoes.first().cloned().unwrap_or_else(|| "Daily Trainer".to_string());
                let default_title = match goal_type {
                    GoalType::CouchTo5k => "C25K Run-Walk Progression",
                    GoalType::ReturnToRunning | GoalType::ReturnFromInjury => "Run-Walk Re-Entry",
                    _ => "Aerobic Base Run",
                };
                let title = today_session
                    .map(|s| s.session_type.display_name().to_string())
                    .unwrap_or_else(|| default_title.to_string());
                (
                    "🟢 GREEN - CLEARED TO TRAIN",
                    "RUN",
                    title,
                    dist,
                    dur,
                    shoe,
                    hr_cap,
                    "172-178 spm",
                    format!("{dist:.1} km aerobic session within HR <= {hr_cap} bpm."),
                )
            };

        let mut audit_summary = if run_permitted {
            format!(
                "Gatekeeper clear ({} caution flag(s)). Rolling 7d: {rolling_7d_km:.1} km.",
                caution_flags.len()
            )
        } else {
            format!(
                "Gatekeeper blocked running ({} hard block(s)). Rolling 7d: {rolling_7d_km:.1} km.",
                hard_blocks.len()
            )
        };

        let mut checklist = Vec::new();
        if run_permitted {
            checklist.push(format!("Maintain HR <= {hr_ceiling_bpm} bpm and cadence >= 170 spm."));
            if input.athlete_config.medical.enforce_pace_floor {
                if let Some(pace_sec) = input.athlete_config.medical.min_pace_sec_per_km {
                    checklist.push(format!(
                        "Respect pace floor >= {}/km.",
                        AthleteConfig::format_pace(pace_sec)
                    ));
                }
            }
            if !eligible_shoes.is_empty() {
                checklist.push(format!("Eligible footwear: {}.", eligible_shoes.join(", ")));
            }
        } else {
            checklist.push("Adhere to gatekeeper running prohibition.".to_string());
            let walk_cap = input.plan.and_then(|p| p.envelope.walk_hr_cap_bpm).unwrap_or(110);
            checklist.push(format!("Optional walking must stay <= {walk_cap} bpm."));
        }
        if is_hybrid {
            audit_summary.push_str(&format!(
                " Hybrid 7d load: {hybrid_sessions_7d_count} strength/erg session(s)."
            ));
            checklist.push(format!(
                "Hybrid spacing: high-strain strength/erg sessions count toward the {}h structural spacing rule.",
                min_spacing_hours as i32
            ));
        } else if run_permitted && goal_type == GoalType::CouchTo5k {
            checklist.push(
                "C25K protocol: follow gradual run-walk intervals and keep 48h recovery spacing between sessions."
                    .to_string(),
            );
        } else if run_permitted && matches!(goal_type, GoalType::ReturnToRunning | GoalType::ReturnFromInjury) {
            checklist.push(
                "Re-entry protocol: stop before sharp discomfort, use walk breaks as needed, and respect 48h spacing between runs."
                    .to_string(),
            );
        }

        let planned_session_snapshot = today_session.map(|s| PlannedSessionSnapshot {
            id: s.id.clone(),
            date: s.date.clone(),
            session_type: s.session_type.as_str().to_string(),
            target: s.target.clone(),
            hr_cap_bpm: s.hr_cap_bpm,
            shoe_constraints: s.shoe_constraints.clone(),
            alternatives: s.alternatives.clone(),
        });

        let readiness_state = ReadinessState {
            version: 1,
            updated_at: updated_at_str.clone(),
            updated_by: "splits-win-engine".to_string(),
            date: today_str.clone(),
            gatekeeper: ReadinessGatekeeper {
                run_permitted,
                hard_blocks: hard_blocks.clone(),
                caution_flags: caution_flags.clone(),
                hours_since_last_run: hours_since_last_run.unwrap_or(999.0),
                rolling_7d_km,
                rolling_7d_runs,
                remaining_weekly_budget_km: remaining_7d_budget_km,
                effective_today_max_km,
            },
            plan_context: ReadinessPlanContext {
                source: if input.plan.is_some() {
                    "shared_plan_json".to_string()
                } else {
                    "self_contained_config".to_string()
                },
                revision: input.plan.map_or(0, |p| p.revision),
                mesocycle_phase: input.plan.map_or_else(
                    || format!("{:?}", input.athlete_config.objective.goal_type).to_lowercase(),
                    |p| p.envelope.mesocycle_phase.clone(),
                ),
                walk_hr_cap_bpm: input.plan.and_then(|p| p.envelope.walk_hr_cap_bpm).unwrap_or(110),
                today_planned_session: planned_session_snapshot,
            },
            shoe_eligibility: ReadinessShoeEligibility {
                source: Some(if input.gear_inventory.is_some() {
                    "gear_inventory.json".to_string()
                } else {
                    "synced_activities".to_string()
                }),
                revision: input.gear_inventory.map(|g| g.revision),
                eligible_run_shoes: eligible_shoes,
                gated_shoes,
                blocked_shoes,
                non_run_footwear: Vec::new(),
                plan_shoe_constraints: today_session.map(|s| s.shoe_constraints.clone()).unwrap_or_default(),
            },
            telemetry_provenance: TelemetryProvenance {
                source: "garmin".to_string(),
                date: today_str.clone(),
            },
            prescription_summary: PrescriptionSummary {
                decision: decision.to_string(),
                distance_km,
                duration_mins,
                shoe_platform: shoe_platform.clone(),
                hr_ceiling_bpm,
            },
        };

        let readiness_summary_text = format!(
            "{status_badge}: {prescribed_title} ({distance_km:.1} km, HR <= {hr_ceiling_bpm} bpm). {audit_summary}"
        );

        let authoritative_target = AuthoritativeTarget {
            generated_at: Some(updated_at_str),
            evaluation_date: Some(today_str),
            day_of_week: Some(day_of_week_str),
            overall_readiness_summary: Some(readiness_summary_text),
            deterministic_metrics: Some(DeterministicMetrics {
                run_permitted_by_gatekeeper: run_permitted,
                hours_since_last_run,
                rolling_7d_km: Some(rolling_7d_km),
                rolling_7d_runs: Some(rolling_7d_runs),
                remaining_7d_budget_km: Some(remaining_7d_budget_km),
                hard_blocks,
                caution_flags,
            }),
            prescription: Some(Prescription {
                status_badge: Some(status_badge.to_string()),
                decision: Some(decision.to_string()),
                prescribed_title: Some(prescribed_title),
                distance_km: Some(distance_km),
                duration_mins: Some(duration_mins),
                shoe_platform: Some(shoe_platform),
                hr_ceiling_bpm: Some(hr_ceiling_bpm),
                cadence_target_spm: Some(cadence_target_spm.to_string()),
                execution_structure: Some(execution_structure),
                autonomic_and_load_audit: Some(audit_summary),
                biomechanical_guardrails_checklist: Some(checklist),
            }),
            fitbit_observation: None,
        };

        ReadinessOutput {
            readiness_state,
            authoritative_target,
        }
    }
}

fn is_running_type(type_key: &str) -> bool {
    let lower = type_key.to_lowercase();
    lower.contains("run") || lower == "trail_running" || lower == "treadmill_running"
}

pub fn is_high_strain_session(activity: &ActivityRecord) -> bool {
    if is_running_type(&activity.type_key) {
        return true;
    }
    let lower = activity.type_key.to_lowercase();
    let is_hybrid_type = matches!(
        lower.as_str(),
        "strength_training" | "hiit" | "cardio" | "indoor_rowing"
    );
    if !is_hybrid_type {
        return false;
    }
    activity.training_load.unwrap_or(0.0) >= 80.0
}

fn parse_iso_utc(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc));
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc));
    }
    None
}

fn resolve_weekly_volume_cap_km(plan: Option<&PlanFile>, config: &AthleteConfig) -> f64 {
    if let Some(p) = plan {
        if p.envelope.max_weekly_km > 0.0 {
            return p.envelope.max_weekly_km;
        }
    }
    if config.objective.goal_type == GoalType::CouchTo5k {
        return 15.0;
    }
    if matches!(
        config.objective.goal_type,
        GoalType::ReturnToRunning | GoalType::ReturnFromInjury
    ) {
        return 18.0;
    }
    if let Some(max_weekday_min) = config.availability.max_weekday_minutes {
        if max_weekday_min > 0 {
            let days_count = config.availability.available_days.len().max(1) as f64;
            let est = days_count * (max_weekday_min as f64 / 6.5) * 0.75;
            return round2(est.clamp(12.0, 60.0));
        }
    }
    20.0
}

fn resolve_shoe_eligibility(
    gear_inventory: Option<&GearInventoryFile>,
    activities: &[&ActivityRecord],
) -> (Vec<String>, Vec<String>, Vec<String>) {
    if let Some(inv) = gear_inventory {
        if !inv.items.is_empty() {
            let mut eligible = Vec::new();
            let mut gated = Vec::new();
            let mut blocked = Vec::new();
            for item in inv.items.values() {
                let name = item.display_name();
                match item.footwear_status() {
                    "eligible" => eligible.push(name),
                    "gated" => gated.push(name),
                    _ => blocked.push(name),
                }
            }
            eligible.sort();
            gated.sort();
            blocked.sort();
            if eligible.is_empty() && gated.is_empty() && blocked.is_empty() {
                eligible.push("Daily Trainer".to_string());
            }
            return (eligible, gated, blocked);
        }
    }

    let mut seen = HashSet::new();
    let mut from_acts = Vec::new();
    for act in activities {
        if let Some(shoe) = act.shoe_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if seen.insert(shoe.to_lowercase()) {
                from_acts.push(shoe.to_string());
            }
        }
    }
    if from_acts.is_empty() {
        from_acts.push("Daily Trainer".to_string());
    }
    (from_acts, Vec::new(), Vec::new())
}

fn round2(val: f64) -> f64 {
    (val * 100.0).round() / 100.0
}
