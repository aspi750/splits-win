use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const APP_SERVICE: &str = "com.aspi.splits-coach";
pub const GARMIN_ACCOUNT: &str = "garmin-personal-api-key";
pub const GEMINI_ACCOUNT: &str = "gemini_api_key";
pub const UISERVER_ACCOUNT: &str = "uiserver-auth-token";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    #[default]
    SelfContained,
    ExternalServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoachEngineMode {
    GeminiApi,
    ClipboardPacket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalType {
    Race,
    BaseBuilding,
    #[default]
    GeneralFitness,
    ReturnFromInjury,
}

impl GoalType {
    pub fn display_name(&self) -> &'static str {
        match self {
            GoalType::Race => "Target Race",
            GoalType::BaseBuilding => "Base Building",
            GoalType::GeneralFitness => "General Fitness",
            GoalType::ReturnFromInjury => "Return from Injury",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AthleteSpecs {
    pub display_name: String,
    pub weight_kg: Option<f64>,
    #[serde(rename = "maxHR")]
    pub max_hr: Option<i32>,
    #[serde(rename = "restingHRBaseline")]
    pub resting_hr_baseline: Option<i32>,
    #[serde(rename = "vt1CeilingBPM")]
    pub vt1_ceiling_bpm: Option<i32>,
    pub timezone_identifier: String,
}

impl Default for AthleteSpecs {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            weight_kg: None,
            max_hr: None,
            resting_hr_baseline: None,
            vt1_ceiling_bpm: None,
            timezone_identifier: "UTC".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AthleteMedical {
    pub conditions_and_notes: String,
    #[serde(rename = "enforceEasyHRCeiling")]
    pub enforce_easy_hr_ceiling: bool,
    #[serde(rename = "easyHRCeilingBPM")]
    pub easy_hr_ceiling_bpm: Option<i32>,
    pub enforce_pace_floor: bool,
    pub min_pace_sec_per_km: Option<i32>,
    pub min_rest_hours_between_runs: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AthleteObjective {
    pub goal_type: GoalType,
    pub race_name: Option<String>,
    pub race_distance_km: Option<f64>,
    pub race_date: Option<String>,
    pub target_time_description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AthleteAvailability {
    pub available_days: Vec<i32>,
    pub preferred_long_run_day: Option<i32>,
    pub max_weekday_minutes: Option<i32>,
    pub cross_training_notes: String,
}

impl Default for AthleteAvailability {
    fn default() -> Self {
        Self {
            available_days: vec![1, 2, 3, 4, 5, 6, 7],
            preferred_long_run_day: Some(7),
            max_weekday_minutes: None,
            cross_training_notes: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AthleteConfig {
    pub specs: AthleteSpecs,
    pub medical: AthleteMedical,
    pub objective: AthleteObjective,
    pub availability: AthleteAvailability,
    pub updated_at: String,
}

impl Default for AthleteConfig {
    fn default() -> Self {
        Self {
            specs: AthleteSpecs::default(),
            medical: AthleteMedical::default(),
            objective: AthleteObjective::default(),
            availability: AthleteAvailability::default(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }
}

impl AthleteConfig {
    /// Computes the median of valid resting HR entries across the trailing 30 days,
    /// requiring at least 5 valid days (`CONTRACT.md §2.1`).
    pub fn calibrated_resting_hr(samples: &[(DateTime<Utc>, Option<i32>)], reference_date: DateTime<Utc>) -> Option<i32> {
        let window_start = reference_date - Duration::days(30);
        let mut valid: Vec<i32> = samples
            .iter()
            .filter_map(|(date, rhr)| {
                if *date >= window_start && *date <= reference_date {
                    rhr.filter(|&v| v > 0)
                } else {
                    None
                }
            })
            .collect();
        if valid.len() < 5 {
            return None;
        }
        valid.sort_unstable();
        let mid = valid.len() / 2;
        if valid.len() % 2 == 0 {
            Some(((valid[mid - 1] + valid[mid]) as f64 / 2.0).round() as i32)
        } else {
            Some(valid[mid])
        }
    }

    pub fn effective_resting_hr_baseline(&self, samples: &[(DateTime<Utc>, Option<i32>)], reference_date: DateTime<Utc>) -> Option<i32> {
        if let Some(explicit) = self.specs.resting_hr_baseline.filter(|&v| v > 0) {
            return Some(explicit);
        }
        Self::calibrated_resting_hr(samples, reference_date)
    }

    pub fn format_pace(seconds_per_km: i32) -> String {
        let mins = seconds_per_km / 60;
        let secs = seconds_per_km % 60;
        format!("{mins}:{secs:02}")
    }

    pub fn iso_weekday_short_name(iso_day: i32) -> Option<&'static str> {
        match iso_day {
            1 => Some("Mon"),
            2 => Some("Tue"),
            3 => Some("Wed"),
            4 => Some("Thu"),
            5 => Some("Fri"),
            6 => Some("Sat"),
            7 => Some("Sun"),
            _ => None,
        }
    }

    pub fn prompt_summary(&self, effective_rhr: Option<i32>) -> String {
        let mut lines = Vec::new();
        let name = self.specs.display_name.trim();
        if !name.is_empty() {
            lines.push(format!("- Athlete: {name}"));
        }
        if let Some(w) = self.specs.weight_kg.filter(|&w| w > 0.0) {
            lines.push(format!("- Weight: {w:.1} kg"));
        }
        if let Some(m) = self.specs.max_hr.filter(|&m| m > 0) {
            lines.push(format!("- Max HR: {m} bpm"));
        }
        if let Some(rhr) = effective_rhr.or(self.specs.resting_hr_baseline).filter(|&r| r > 0) {
            let source = if self.specs.resting_hr_baseline.is_some() { "configured" } else { "30d median" };
            lines.push(format!("- Resting HR baseline: {rhr} bpm ({source})"));
        }
        if let Some(vt1) = self.specs.vt1_ceiling_bpm.filter(|&v| v > 0) {
            lines.push(format!("- VT1 aerobic ceiling: {vt1} bpm"));
        }
        lines.push(format!("- Timezone: {}", self.specs.timezone_identifier));

        let notes = self.medical.conditions_and_notes.trim();
        if !notes.is_empty() {
            lines.push(format!("- Medical & injury notes: {notes}"));
        }
        if self.medical.enforce_easy_hr_ceiling {
            if let Some(c) = self.medical.easy_hr_ceiling_bpm.filter(|&v| v > 0) {
                lines.push(format!("- Enforced easy HR ceiling: {c} bpm"));
            }
        }
        if self.medical.enforce_pace_floor {
            if let Some(p) = self.medical.min_pace_sec_per_km.filter(|&v| v > 0) {
                lines.push(format!("- Enforced pace floor: {}/km (do not run faster)", Self::format_pace(p)));
            }
        }
        if let Some(rest) = self.medical.min_rest_hours_between_runs.filter(|&v| v > 0) {
            lines.push(format!("- Minimum rest between runs: {rest} hours"));
        }

        let mut goal_parts = vec![self.objective.goal_type.display_name().to_string()];
        if let Some(rn) = self.objective.race_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            goal_parts.push(rn.to_string());
        }
        if let Some(dist) = self.objective.race_distance_km.filter(|&d| d > 0.0) {
            goal_parts.push(format!("{dist:.1} km"));
        }
        if let Some(t) = self.objective.target_time_description.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            goal_parts.push(format!("target: {t}"));
        }
        lines.push(format!("- Objective: {}", goal_parts.join(" · ")));

        if !self.availability.available_days.is_empty() {
            let mut days = self.availability.available_days.clone();
            days.sort_unstable();
            let names: Vec<&str> = days.into_iter().filter_map(Self::iso_weekday_short_name).collect();
            if !names.is_empty() {
                lines.push(format!("- Available training days: {}", names.join(", ")));
            }
        }
        if let Some(ld) = self.availability.preferred_long_run_day.and_then(Self::iso_weekday_short_name) {
            lines.push(format!("- Preferred long run day: {ld}"));
        }
        if let Some(mm) = self.availability.max_weekday_minutes.filter(|&v| v > 0) {
            lines.push(format!("- Max weekday session duration: {mm} min"));
        }
        let cross = self.availability.cross_training_notes.trim();
        if !cross.is_empty() {
            lines.push(format!("- Cross-training notes: {cross}"));
        }
        lines.join("\n")
    }
}

// MARK: - ReadinessState & AuthoritativeTarget (`CONTRACT.md §2.2-2.3`)

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadinessState {
    pub version: i32,
    pub updated_at: String,
    pub updated_by: String,
    pub date: String,
    pub gatekeeper: ReadinessGatekeeper,
    pub plan_context: ReadinessPlanContext,
    pub shoe_eligibility: ReadinessShoeEligibility,
    pub telemetry_provenance: TelemetryProvenance,
    pub prescription_summary: PrescriptionSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadinessGatekeeper {
    pub run_permitted: bool,
    pub hard_blocks: Vec<String>,
    pub caution_flags: Vec<String>,
    pub hours_since_last_run: f64,
    pub rolling_7d_km: f64,
    pub rolling_7d_runs: i32,
    pub remaining_weekly_budget_km: f64,
    pub effective_today_max_km: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadinessPlanContext {
    pub source: String,
    pub revision: i32,
    pub mesocycle_phase: String,
    pub walk_hr_cap_bpm: i32,
    pub today_planned_session: Option<PlannedSessionSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedSessionSnapshot {
    pub id: String,
    pub date: String,
    #[serde(rename = "type")]
    pub session_type: String,
    pub target: PlanTargetRange,
    pub hr_cap_bpm: i32,
    pub shoe_constraints: Vec<String>,
    pub alternatives: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanTargetRange {
    pub duration_min: Vec<f64>,
    pub distance_km: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadinessShoeEligibility {
    pub source: Option<String>,
    pub revision: Option<i32>,
    pub eligible_run_shoes: Vec<String>,
    pub gated_shoes: Vec<String>,
    pub blocked_shoes: Vec<String>,
    pub non_run_footwear: Vec<String>,
    pub plan_shoe_constraints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryProvenance {
    pub source: String,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrescriptionSummary {
    pub decision: String,
    pub distance_km: f64,
    pub duration_mins: i32,
    pub shoe_platform: String,
    pub hr_ceiling_bpm: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthoritativeTarget {
    pub generated_at: Option<String>,
    pub evaluation_date: Option<String>,
    pub day_of_week: Option<String>,
    pub overall_readiness_summary: Option<String>,
    pub deterministic_metrics: Option<DeterministicMetrics>,
    pub prescription: Option<Prescription>,
    #[serde(alias = "secondary_observation")]
    pub fitbit_observation: Option<SecondaryObservation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeterministicMetrics {
    pub run_permitted_by_gatekeeper: bool,
    pub hours_since_last_run: Option<f64>,
    pub rolling_7d_km: Option<f64>,
    pub rolling_7d_runs: Option<i32>,
    pub remaining_7d_budget_km: Option<f64>,
    pub hard_blocks: Vec<String>,
    pub caution_flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prescription {
    pub status_badge: Option<String>,
    pub decision: Option<String>,
    pub prescribed_title: Option<String>,
    pub distance_km: Option<f64>,
    pub duration_mins: Option<i32>,
    pub shoe_platform: Option<String>,
    pub hr_ceiling_bpm: Option<i32>,
    pub cadence_target_spm: Option<String>,
    pub execution_structure: Option<String>,
    pub autonomic_and_load_audit: Option<String>,
    pub biomechanical_guardrails_checklist: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecondaryObservation {
    pub available: bool,
    pub device_name: Option<String>,
    pub hrv: Option<SecondaryMetric>,
    pub resting_hr: Option<SecondaryMetric>,
    pub sleep: Option<SecondarySleep>,
    pub disclaimer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecondaryMetric {
    pub value: Option<f64>,
    pub unit: Option<String>,
    pub date: Option<String>,
    pub status: Option<String>,
    pub provenance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecondarySleep {
    pub date: Option<String>,
    pub asleep_duration_minutes: Option<f64>,
    pub session_duration_minutes: Option<f64>,
    pub status: Option<String>,
    pub provenance: Option<String>,
}

// MARK: - PlanFile, GearInventoryFile, OutcomesFile, CalibrationFile (`CONTRACT.md §2.4`)

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTypology {
    RunWalkReentry,
    EasyAerobic,
    LongRun,
    HillWork,
    TempoVt2,
    SpeedIntervals,
}

impl RunTypology {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunTypology::RunWalkReentry => "run_walk_reentry",
            RunTypology::EasyAerobic => "easy_aerobic",
            RunTypology::LongRun => "long_run",
            RunTypology::HillWork => "hill_work",
            RunTypology::TempoVt2 => "tempo_vt2",
            RunTypology::SpeedIntervals => "speed_intervals",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            RunTypology::RunWalkReentry => "Run-Walk Re-entry",
            RunTypology::EasyAerobic => "Easy Aerobic (Z2)",
            RunTypology::LongRun => "Long Run",
            RunTypology::HillWork => "Hill Work",
            RunTypology::TempoVt2 => "Tempo (VT2)",
            RunTypology::SpeedIntervals => "Speed Intervals (Z4/Z5)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Valid,
    Provisional,
    Expired,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanDateRange {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanEnvelope {
    pub max_weekly_km: f64,
    pub max_single_run_km: f64,
    pub mesocycle_phase: String,
    pub min_hours_between_runs: f64,
    pub walk_hr_cap_bpm: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanSession {
    pub id: String,
    pub date: String,
    #[serde(rename = "type")]
    pub session_type: RunTypology,
    pub target: PlanTargetRange,
    pub hr_cap_bpm: i32,
    pub shoe_constraints: Vec<String>,
    pub alternatives: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanFile {
    pub version: i32,
    pub revision: i32,
    pub updated_at: String,
    pub updated_by: String,
    pub covers: PlanDateRange,
    pub status: PlanStatus,
    pub envelope: PlanEnvelope,
    pub sessions: Vec<PlanSession>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GearGarminMeta {
    pub name: Option<String>,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub gear_type: Option<String>,
    pub status: Option<String>,
    pub distance_used_m: Option<f64>,
    pub activities_linked: Option<i32>,
    pub max_usage_distance_m: Option<f64>,
    pub first_use: Option<String>,
    pub retrieved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GearEligibility {
    Detailed {
        status: String,
        decided_on: Option<String>,
        decided_with: Option<String>,
        reason: Option<String>,
    },
    Simple(String),
}

impl GearEligibility {
    pub fn status(&self) -> &str {
        match self {
            GearEligibility::Detailed { status, .. } => status,
            GearEligibility::Simple(s) => s,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GearItem {
    pub garmin: Option<GearGarminMeta>,
    pub alias: Option<String>,
    pub eligibility: Option<GearEligibility>,
    pub pending_naming: Option<bool>,
}

impl GearItem {
    pub fn display_name(&self) -> String {
        if let Some(alias) = self.alias.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return alias.to_string();
        }
        if let Some(name) = self.garmin.as_ref().and_then(|g| g.name.as_deref()).map(str::trim).filter(|s| !s.is_empty()) {
            return name.to_string();
        }
        if let Some(model) = self.garmin.as_ref().and_then(|g| g.model.as_deref()).map(str::trim).filter(|s| !s.is_empty()) {
            return model.to_string();
        }
        "Unnamed Shoe".to_string()
    }

    pub fn footwear_status(&self) -> &'static str {
        match self.eligibility.as_ref().map(|e| e.status()) {
            Some("eligible") => "eligible",
            Some("gated") => "gated",
            Some("blocked") | Some("not_eligible") => "blocked",
            _ => {
                if self.garmin.as_ref().and_then(|g| g.status.as_deref()) == Some("RETIRED") {
                    "blocked"
                } else {
                    "gated"
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GearPendingItem {
    pub uuid: String,
    pub first_seen_activity_id: i64,
    pub first_seen_date: String,
    pub asked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GearInventoryFile {
    pub version: i32,
    pub revision: i32,
    pub updated_at: String,
    pub updated_by: String,
    pub items: HashMap<String, GearItem>,
    pub pending: Vec<GearPendingItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityRecord {
    pub garmin_id: i64,
    pub name: String,
    pub type_key: String,
    pub start_iso: String,
    pub distance_m: f64,
    pub duration_sec: f64,
    pub moving_duration_sec: f64,
    pub elevation_gain_m: f64,
    pub average_hr: Option<f64>,
    pub max_hr: Option<f64>,
    pub cadence_spm: Option<f64>,
    pub training_load: Option<f64>,
    pub shoe_name: Option<String>,
}

impl ActivityRecord {
    pub fn distance_km(&self) -> f64 {
        self.distance_m / 1000.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayRecord {
    pub date: String,
    pub resting_hr: Option<i32>,
    pub stress_avg: Option<i32>,
    pub body_battery_high: Option<i32>,
    pub body_battery_latest: Option<i32>,
    pub steps: Option<i32>,
    pub step_goal: Option<i32>,
    pub sleep_seconds: Option<i32>,
    pub sleep_score: Option<i32>,
    pub hrv_last_night: Option<i32>,
    pub hrv_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoachUsageEntry {
    pub timestamp: String,
    pub profile: String,
    pub model: String,
    pub prompt_tokens: i32,
    pub candidates_tokens: i32,
    pub thoughts_tokens: i32,
    pub total_tokens: i32,
    pub finish_reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailabilityEntry {
    pub key: String,
    pub value: String,
    pub valid_from: String,
    pub valid_until: String,
    pub source: String,
    pub last_confirmed: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoachUpdateProposal {
    pub id: String,
    pub target_file: String,
    pub reason: String,
    pub updated_section_markdown: String,
    pub expires_on: Option<String>,
    pub diff_preview: String,
    pub status: String,
}
