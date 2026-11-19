use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const APP_SERVICE: &str = "com.aspi.splits-coach";
pub const GARMIN_KEYRING_SERVICE: &str = "splits-coach-garmin-api-key";
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
    HybridHyrox,
}

impl GoalType {
    pub fn display_name(&self) -> &'static str {
        match self {
            GoalType::Race => "Target Race",
            GoalType::BaseBuilding => "Base Building",
            GoalType::GeneralFitness => "General Fitness",
            GoalType::ReturnFromInjury => "Return from Injury",
            GoalType::HybridHyrox => "Hyrox / Hybrid Athlete",
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
pub struct HybridProfile {
    pub division: Option<String>,
    #[serde(default)]
    pub focus_stations: Vec<String>,
    pub weekly_hybrid_sessions_target: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AthleteObjective {
    pub goal_type: GoalType,
    pub race_name: Option<String>,
    pub race_distance_km: Option<f64>,
    pub race_date: Option<String>,
    pub target_time_description: Option<String>,
    #[serde(default)]
    pub hybrid_profile: Option<HybridProfile>,
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

        if self.objective.goal_type == GoalType::HybridHyrox {
            if let Some(ref hybrid) = self.objective.hybrid_profile {
                let mut hybrid_parts = Vec::new();
                if let Some(div) = hybrid.division.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    hybrid_parts.push(format!("Division: {div}"));
                }
                if !hybrid.focus_stations.is_empty() {
                    hybrid_parts.push(format!("Focus stations: {}", hybrid.focus_stations.join(", ")));
                }
                if let Some(target) = hybrid.weekly_hybrid_sessions_target.filter(|&v| v > 0) {
                    hybrid_parts.push(format!("Hybrid sessions/wk: {target}"));
                }
                if !hybrid_parts.is_empty() {
                    lines.push(format!("- Hybrid profile: {}", hybrid_parts.join(" · ")));
                }
            }
        }

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

pub fn default_temp_source() -> String {
    "watch_internal".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OrthopedicCheckIn {
    pub left_achilles: i32,
    pub lumbar_sciatica: i32,
    pub right_ankle: i32,
    pub calves_soleus: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PostRunContext {
    pub shoe_model: String,
    pub shoe_size: String,
    pub pre_run_weight_kg: Option<f64>,
    pub post_run_weight_kg: Option<f64>,
    pub fluid_consumed_ml: Option<f64>,
    pub net_fluid_deficit_kg: Option<f64>,
    pub net_sweat_rate_l_hr: Option<f64>,
    pub orthopedic: OrthopedicCheckIn,
    #[serde(default)]
    pub pre_weight_manually_edited: bool,
    #[serde(default)]
    pub post_weight_manually_edited: bool,
    #[serde(default)]
    pub fluid_intake_confirmed: bool,
    #[serde(default)]
    pub weight_source: Option<String>,
    #[serde(default)]
    pub sweat_status: Option<String>,
    #[serde(default)]
    pub provisional_sweat_rate_l_hr: Option<f64>,
    #[serde(default)]
    pub final_sweat_rate_l_hr: Option<f64>,
    #[serde(default)]
    pub tempe_sensor_used: Option<bool>,
}

impl PostRunContext {
    pub fn effective_temp_source(&self, fit_temp_source: Option<&str>) -> String {
        if fit_temp_source == Some("garmin_tempe_fit") {
            return "garmin_tempe_fit".to_string();
        }
        if self.tempe_sensor_used == Some(true) {
            return "user_confirmed_tempe".to_string();
        }
        "watch_internal".to_string()
    }

    pub fn with_computed_sweat(mut self, duration_sec: f64) -> Self {
        if let (Some(pre), Some(post)) = (self.pre_run_weight_kg, self.post_run_weight_kg) {
            if pre > 0.0 && post > 0.0 {
                let prov_deficit = ((pre - post) * 100.0).round() / 100.0;
                let prov_rate = if duration_sec > 60.0 {
                    let hrs = duration_sec / 3600.0;
                    Some(((prov_deficit / hrs) * 100.0).round() / 100.0)
                } else {
                    None
                };
                self.provisional_sweat_rate_l_hr = prov_rate;

                if self.fluid_intake_confirmed {
                    let fluid_l = self.fluid_consumed_ml.unwrap_or(0.0) / 1000.0;
                    let fin_deficit = ((pre - post + fluid_l) * 100.0).round() / 100.0;
                    self.net_fluid_deficit_kg = Some(fin_deficit);
                    if duration_sec > 60.0 {
                        let hrs = duration_sec / 3600.0;
                        let rate = ((fin_deficit / hrs) * 100.0).round() / 100.0;
                        self.final_sweat_rate_l_hr = Some(rate);
                        self.net_sweat_rate_l_hr = Some(rate);
                    } else {
                        self.final_sweat_rate_l_hr = None;
                        self.net_sweat_rate_l_hr = None;
                    }
                    self.sweat_status = Some("final_verified".to_string());
                } else {
                    self.net_fluid_deficit_kg = Some(prov_deficit);
                    self.final_sweat_rate_l_hr = None;
                    self.net_sweat_rate_l_hr = prov_rate;
                    self.sweat_status = Some("provisional_no_intake".to_string());
                }
                return self;
            }
        }
        self.net_fluid_deficit_kg = None;
        self.net_sweat_rate_l_hr = None;
        self.provisional_sweat_rate_l_hr = None;
        self.final_sweat_rate_l_hr = None;
        self.sweat_status = None;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeightEntry {
    pub timestamp_epoch_sec: i64,
    pub timestamp_iso: String,
    pub weight_kg: f64,
    pub body_fat_pct: Option<f64>,
    pub body_water_pct: Option<f64>,
    pub muscle_mass_kg: Option<f64>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedWeight {
    pub weight_kg: f64,
    pub source: String,
    pub is_provisional: bool,
}

pub mod scale_matcher {
    use super::{ActivityRecord, PostRunContext, ResolvedWeight, WeightEntry};
    use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};

    pub fn parse_activity_epoch_sec(raw: &str) -> Option<i64> {
        let trimmed = raw.trim();
        if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
            return Some(dt.with_timezone(&Utc).timestamp());
        }
        let prefix = trimmed.chars().take(19).collect::<String>();
        for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
            if let Ok(ndt) = NaiveDateTime::parse_from_str(&prefix, fmt) {
                if let Some(local_dt) = Local.from_local_datetime(&ndt).single() {
                    return Some(local_dt.with_timezone(&Utc).timestamp());
                }
                return Some(DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc).timestamp());
            }
        }
        None
    }

    pub fn activity_windows(activities: &[ActivityRecord]) -> Vec<(i64, i64)> {
        activities
            .iter()
            .filter_map(|a| {
                let start = parse_activity_epoch_sec(&a.start_iso)?;
                let dur = a.duration_sec.max(a.moving_duration_sec).max(0.0).round() as i64;
                Some((start, start + dur))
            })
            .collect()
    }

    pub fn is_post_run_dehydrated(weigh_in_epoch: i64, windows: &[(i64, i64)]) -> bool {
        windows
            .iter()
            .any(|&(_start, end)| weigh_in_epoch >= end - 5 * 60 && weigh_in_epoch <= end + 90 * 60)
    }

    /// Returns the newest morning / pre-run Garmin Scale weight, never updating from dehydrated post-run weigh-ins.
    pub fn latest_baseline_weight_kg(
        weigh_ins: &[WeightEntry],
        windows: &[(i64, i64)],
    ) -> Option<f64> {
        weigh_ins
            .iter()
            .filter(|w| w.weight_kg > 0.0 && !is_post_run_dehydrated(w.timestamp_epoch_sec, windows))
            .max_by_key(|w| w.timestamp_epoch_sec)
            .map(|w| w.weight_kg)
    }

    pub fn pair_scale_weigh_ins(
        activity_start_epoch: i64,
        activity_end_epoch: i64,
        duration_sec: f64,
        weigh_ins: &[WeightEntry],
        existing: PostRunContext,
    ) -> PostRunContext {
        let mut ctx = existing;
        let pre_lo = activity_start_epoch - 120 * 60;
        let pre_hi = activity_start_epoch + 10 * 60;
        if !ctx.pre_weight_manually_edited {
            if let Some(pre) = weigh_ins
                .iter()
                .filter(|w| {
                    w.source == "garmin_scale"
                        && w.weight_kg > 0.0
                        && w.timestamp_epoch_sec >= pre_lo
                        && w.timestamp_epoch_sec <= pre_hi
                })
                .min_by_key(|w| (w.timestamp_epoch_sec - activity_start_epoch).abs())
            {
                ctx.pre_run_weight_kg = Some(pre.weight_kg);
                ctx.weight_source = Some("garmin_scale".to_string());
            }
        }

        let post_lo = activity_end_epoch - 5 * 60;
        let post_hi = activity_end_epoch + 90 * 60;
        if !ctx.post_weight_manually_edited {
            if let Some(post) = weigh_ins
                .iter()
                .filter(|w| {
                    w.source == "garmin_scale"
                        && w.weight_kg > 0.0
                        && w.timestamp_epoch_sec >= post_lo
                        && w.timestamp_epoch_sec <= post_hi
                })
                .min_by_key(|w| (w.timestamp_epoch_sec - activity_end_epoch).abs())
            {
                ctx.post_run_weight_kg = Some(post.weight_kg);
                if ctx.weight_source.is_none() {
                    ctx.weight_source = Some("garmin_scale".to_string());
                }
            }
        }

        ctx.with_computed_sweat(duration_sec)
    }

    pub fn resolve_effective_weight(
        post_run: Option<&PostRunContext>,
        ref_epoch: i64,
        weigh_ins: &[WeightEntry],
        windows: &[(i64, i64)],
        profile_weight_kg: Option<f64>,
    ) -> Option<ResolvedWeight> {
        if let Some(ctx) = post_run {
            if let Some(pre) = ctx.pre_run_weight_kg.filter(|&w| w > 0.0) {
                let source = if ctx.pre_weight_manually_edited {
                    "manual".to_string()
                } else {
                    ctx.weight_source.clone().unwrap_or_else(|| "manual".to_string())
                };
                return Some(ResolvedWeight {
                    weight_kg: pre,
                    source,
                    is_provisional: false,
                });
            }
        }
        let cutoff = ref_epoch - 14 * 86_400;
        if let Some(recent) = weigh_ins
            .iter()
            .filter(|w| {
                w.weight_kg > 0.0
                    && w.timestamp_epoch_sec >= cutoff
                    && w.timestamp_epoch_sec <= ref_epoch
                    && !is_post_run_dehydrated(w.timestamp_epoch_sec, windows)
            })
            .max_by_key(|w| w.timestamp_epoch_sec)
        {
            return Some(ResolvedWeight {
                weight_kg: recent.weight_kg,
                source: "latest_14d_weigh_in".to_string(),
                is_provisional: true,
            });
        }
        if let Some(base) = profile_weight_kg.filter(|&w| w > 0.0) {
            return Some(ResolvedWeight {
                weight_kg: base,
                source: "profile_baseline".to_string(),
                is_provisional: true,
            });
        }
        None
    }
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
    #[serde(default)]
    pub is_hyrox: Option<bool>,
    #[serde(default)]
    pub post_run_context: Option<PostRunContext>,
    #[serde(default)]
    pub fit_temp_source: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FitTrackPoint {
    pub timestamp_sec: f64,
    pub distance_m: f64,
    pub speed_mps: f64,
    pub pace_sec_per_km: Option<f64>,
    pub heart_rate_bpm: Option<f64>,
    pub cadence_spm: Option<f64>,
    pub altitude_m: Option<f64>,
    #[serde(default)]
    pub power_w: Option<f64>,
    #[serde(default)]
    pub gct_ms: Option<f64>,
    #[serde(default)]
    pub stance_time_pct: Option<f64>,
    #[serde(default)]
    pub vertical_oscillation_mm: Option<f64>,
    #[serde(default)]
    pub vertical_ratio_pct: Option<f64>,
    #[serde(default)]
    pub step_length_mm: Option<f64>,
    #[serde(default)]
    pub stance_balance_left_pct: Option<f64>,
    #[serde(default)]
    pub temperature_c: Option<f64>,
    #[serde(default)]
    pub respiration_brpm: Option<f64>,
    #[serde(default)]
    pub latitude_deg: Option<f64>,
    #[serde(default)]
    pub longitude_deg: Option<f64>,
    #[serde(default)]
    pub impact_load: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FitSplit {
    pub km_index: i32,
    pub distance_m: f64,
    pub duration_sec: f64,
    pub pace_sec_per_km: Option<f64>,
    pub avg_hr_bpm: Option<f64>,
    pub avg_cadence_spm: Option<f64>,
    pub elevation_gain_m: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FitGuardrailAudit {
    pub pace_floor_sec_per_km: Option<i32>,
    pub easy_hr_ceiling_bpm: Option<i32>,
    pub pace_floor_violations: i32,
    pub hr_ceiling_violations: i32,
    pub compliant: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FitLap {
    pub elapsed_sec: f64,
    pub moving_sec: f64,
    pub distance_m: f64,
    pub avg_hr_bpm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompromisedRunAudit {
    pub run_segment_count: usize,
    pub first_half_avg_pace_sec_per_km: f64,
    pub second_half_avg_pace_sec_per_km: f64,
    pub pace_fade_sec_per_km: f64,
    pub first_half_avg_hr: Option<f64>,
    pub second_half_avg_hr: Option<f64>,
    pub hr_drift_bpm: Option<f64>,
}

impl CompromisedRunAudit {
    pub fn compute(laps: &[FitLap]) -> Option<Self> {
        struct Seg {
            is_run: bool,
            distance: f64,
            seconds: f64,
            avg_hr: Option<f64>,
        }
        let segments: Vec<Seg> = laps
            .iter()
            .filter_map(|lap| {
                let sec = if lap.moving_sec > 0.0 { lap.moving_sec } else { lap.elapsed_sec };
                if sec <= 0.0 {
                    return None;
                }
                let pace = if lap.distance_m > 0.0 {
                    sec / (lap.distance_m / 1000.0)
                } else {
                    f64::INFINITY
                };
                let is_run = lap.distance_m >= 600.0 && pace <= 600.0;
                Some(Seg {
                    is_run,
                    distance: lap.distance_m,
                    seconds: sec,
                    avg_hr: lap.avg_hr_bpm,
                })
            })
            .collect();
        if segments.len() < 4 {
            return None;
        }
        let runs: Vec<&Seg> = segments.iter().filter(|s| s.is_run).collect();
        let stations = segments.len() - runs.len();
        if runs.len() < 2 || stations < 2 {
            return None;
        }
        let mut transitions = 0usize;
        for i in 1..segments.len() {
            if segments[i].is_run != segments[i - 1].is_run {
                transitions += 1;
            }
        }
        if transitions < 3 {
            return None;
        }
        let mid = runs.len() / 2;
        let first = &runs[..mid];
        let second = &runs[mid..];

        let calc_pace = |segs: &[&Seg]| -> f64 {
            let meters: f64 = segs.iter().map(|s| s.distance).sum();
            let secs: f64 = segs.iter().map(|s| s.seconds).sum();
            if meters > 0.0 { secs / (meters / 1000.0) } else { 0.0 }
        };
        let calc_hr = |segs: &[&Seg]| -> Option<f64> {
            let vals: Vec<f64> = segs.iter().filter_map(|s| s.avg_hr).collect();
            if vals.is_empty() { None } else { Some(vals.iter().sum::<f64>() / vals.len() as f64) }
        };
        let r1 = |v: f64| (v * 10.0).round() / 10.0;

        let first_pace = r1(calc_pace(first));
        let second_pace = r1(calc_pace(second));
        let first_hr = calc_hr(first).map(r1);
        let second_hr = calc_hr(second).map(r1);
        let drift = match (first_hr, second_hr) {
            (Some(a), Some(b)) => Some(r1(b - a)),
            _ => None,
        };

        Some(Self {
            run_segment_count: runs.len(),
            first_half_avg_pace_sec_per_km: first_pace,
            second_half_avg_pace_sec_per_km: second_pace,
            pace_fade_sec_per_km: r1(second_pace - first_pace),
            first_half_avg_hr: first_hr,
            second_half_avg_hr: second_hr,
            hr_drift_bpm: drift,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AthleteType {
    #[default]
    Runner,
    Hyrox,
    HybridBoth,
}

impl AthleteType {
    pub fn as_str(&self) -> &'static str {
        match self {
            AthleteType::Runner => "runner",
            AthleteType::Hyrox => "hyrox",
            AthleteType::HybridBoth => "hybrid_both",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "hyrox" => AthleteType::Hyrox,
            "hybrid_both" => AthleteType::HybridBoth,
            _ => AthleteType::Runner,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RunnerConfig {
    pub primary_distance: Option<String>,
    #[serde(default)]
    pub hr_first_governor: bool,
    pub pace_floor_sec_per_km: Option<i32>,
    pub z2_range_bpm: Option<[i32; 2]>,
    pub z3_range_bpm: Option<[i32; 2]>,
    pub lthr_bpm: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HyroxConfig {
    pub division: Option<String>,
    pub watch_logging_mode: Option<String>,
    pub target_total_time_sec: Option<i32>,
    pub fresh_1km_pace_sec: Option<i32>,
    pub compromised_1km_target_sec: Option<i32>,
    #[serde(default)]
    pub weak_stations: Vec<String>,
}

impl HyroxConfig {
    pub fn official_weights_summary(&self) -> &'static str {
        match self.division.as_deref().unwrap_or("").to_lowercase().as_str() {
            d if d.contains("pro") && d.contains("women") => {
                "Pro Women: Sled Push 152 kg, Sled Pull 103 kg, Farmers Carry 2x24 kg, Sandbag Lunges 20 kg, Wall Balls 6 kg"
            }
            d if d.contains("pro") => {
                "Pro Men: Sled Push 202 kg, Sled Pull 153 kg, Farmers Carry 2x32 kg, Sandbag Lunges 30 kg, Wall Balls 9 kg"
            }
            d if d.contains("women") => {
                "Open Women: Sled Push 102 kg, Sled Pull 78 kg, Farmers Carry 2x16 kg, Sandbag Lunges 10 kg, Wall Balls 4 kg"
            }
            _ => {
                "Open Men / Doubles: Sled Push 152 kg, Sled Pull 103 kg, Farmers Carry 2x24 kg, Sandbag Lunges 20 kg, Wall Balls 6 kg"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UserProfile {
    pub athlete_type: AthleteType,
    pub onboarding_completed: bool,
    pub runner_config: RunnerConfig,
    pub hyrox_config: HyroxConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LapKind {
    Run,
    Station,
    Roxzone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassifiedLap {
    pub lap_index: usize,
    pub kind: LapKind,
    pub elapsed_sec: f64,
    pub moving_sec: f64,
    pub distance_m: f64,
    pub pace_sec_per_km: Option<f64>,
    pub avg_hr_bpm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HyroxDissection {
    pub is_hyrox_sim: bool,
    pub run_splits: Vec<ClassifiedLap>,
    pub station_splits: Vec<ClassifiedLap>,
    pub roxzone_transitions: Vec<ClassifiedLap>,
    pub clean_run_pace_sec_per_km: Option<f64>,
    pub clean_aerobic_efficiency: Option<f64>,
    pub compromised_pace_decay_sec: Option<f64>,
    pub station_hr_recovery_sec: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FitTrackAnalysis {
    pub garmin_id: i64,
    pub chart_points: Vec<FitTrackPoint>,
    pub full_points: Vec<FitTrackPoint>,
    pub splits: Vec<FitSplit>,
    #[serde(default)]
    pub laps: Vec<FitLap>,
    #[serde(default)]
    pub compromised_run_audit: Option<CompromisedRunAudit>,
    #[serde(default)]
    pub hyrox_dissection: Option<HyroxDissection>,
    #[serde(default)]
    pub forensic_telemetry: Option<crate::forensic_extractor::ForensicTelemetryReport>,
    #[serde(default = "default_temp_source")]
    pub temp_source: String,
    pub decoupling_pct: Option<f64>,
    pub audit: FitGuardrailAudit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityDetailPayload {
    pub activity: ActivityRecord,
    pub track: Option<FitTrackAnalysis>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FitBaselineProbe {
    pub scanned_files: usize,
    pub detected_max_hr: Option<i32>,
    pub detected_rhr_baseline: Option<i32>,
    pub estimated_lthr: Option<i32>,
    pub weekly_km_est: Option<f64>,
}
