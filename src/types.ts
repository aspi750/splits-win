export type ExecutionMode = "self_contained" | "external_server";
export type CoachEngineMode = "gemini_api" | "clipboard_packet";
export type ThinkingLevel = "LOW" | "MEDIUM" | "HIGH";

export interface ModelSettings {
  global_model_id: string;
  global_thinking_level: ThinkingLevel;
  coach_model_override_id: string | null;
  coach_thinking_override: ThinkingLevel | null;
}

export type FirstUseStage =
  | "intake"
  | "profile_approval"
  | "history_sync"
  | "plan_approval"
  | "active";

export interface IntakeQueuedFile {
  id: string;
  file_name: string;
  relative_path: string;
  mime_type: string;
  kind: string;
  added_at: string;
}

export interface FirstUseFlowState {
  stage: FirstUseStage;
  auto_calibrate_after_first_sync: boolean;
  queued_intake_files: IntakeQueuedFile[];
  history_summary_markdown: string | null;
  updated_at: string;
}
export type GoalType =
  | "race"
  | "base_building"
  | "general_fitness"
  | "return_from_injury"
  | "return_to_running"
  | "couch_to_5k"
  | "hybrid_hyrox";

export type RaceDistancePreset =
  | "none"
  | "5k"
  | "10k"
  | "half_marathon"
  | "marathon"
  | "ultra_50k"
  | "custom";

export const RACE_DISTANCE_PRESETS: {
  id: RaceDistancePreset;
  label: string;
  km: number | null;
  defaultName: string | null;
}[] = [
  { id: "none", label: "No Specific Distance (Optional)", km: null, defaultName: null },
  { id: "5k", label: "5K (5.0 km)", km: 5.0, defaultName: "5K" },
  { id: "10k", label: "10K (10.0 km)", km: 10.0, defaultName: "10K" },
  { id: "half_marathon", label: "Half Marathon (21.1 km)", km: 21.1, defaultName: "Half Marathon" },
  { id: "marathon", label: "Marathon (42.2 km)", km: 42.2, defaultName: "Marathon" },
  { id: "ultra_50k", label: "50K Ultra (50.0 km)", km: 50.0, defaultName: "50K Ultra" },
  { id: "custom", label: "Custom Distance (km)", km: null, defaultName: null },
];

export function presetDistanceKm(preset: RaceDistancePreset): number | null {
  return RACE_DISTANCE_PRESETS.find((p) => p.id === preset)?.km ?? null;
}

export function presetDefaultName(preset: RaceDistancePreset): string | null {
  return RACE_DISTANCE_PRESETS.find((p) => p.id === preset)?.defaultName ?? null;
}

export function presetFromDistanceKm(
  km: number | null | undefined,
  allowNone = true
): RaceDistancePreset {
  if (!km || km <= 0) {
    return allowNone ? "none" : "half_marathon";
  }
  if (Math.abs(km - 5.0) < 0.05) return "5k";
  if (Math.abs(km - 10.0) < 0.05) return "10k";
  if (Math.abs(km - 21.1) < 0.15) return "half_marathon";
  if (Math.abs(km - 42.2) < 0.15) return "marathon";
  if (Math.abs(km - 50.0) < 0.15) return "ultra_50k";
  return "custom";
}

export function sanitizeHrInput(raw: string): string {
  return raw.replace(/\D/g, "").slice(0, 3);
}

export function parseValidHrBpm(raw: string | number | null | undefined): number | null {
  if (raw === null || raw === undefined) return null;
  const digits = typeof raw === "number" ? String(Math.round(raw)) : sanitizeHrInput(raw);
  if (!digits) return null;
  const n = Number(digits);
  if (!Number.isFinite(n) || n < 30 || n > 240) return null;
  return Math.round(n);
}

export interface HybridProfile {
  division: string | null;
  focusStations: string[];
  weeklyHybridSessionsTarget: number | null;
}

export type TrainingDiscipline = "running" | "hyrox" | "hybrid";

export interface AthleteConfig {
  specs: {
    displayName: string;
    weightKg: number | null;
    weightBaselineConfirmed?: boolean;
    maxHR: number | null;
    restingHRBaseline: number | null;
    vt1CeilingBPM: number | null;
    timezoneIdentifier: string;
  };
  medical: {
    conditionsAndNotes: string;
    enforceEasyHRCeiling: boolean;
    easyHRCeilingBPM: number | null;
    enforcePaceFloor: boolean;
    minPaceSecPerKm: number | null;
    minRestHoursBetweenRuns: number | null;
  };
  objective: {
    goalType: GoalType;
    raceName: string | null;
    raceDistanceKm: number | null;
    raceDate: string | null;
    targetTimeDescription: string | null;
    hybridProfile?: HybridProfile | null;
  };
  availability: {
    availableDays: number[];
    preferredLongRunDay: number | null;
    maxWeekdayMinutes: number | null;
    crossTrainingNotes: string;
  };
  discipline?: TrainingDiscipline;
  updatedAt: string;
}

export interface ReadinessState {
  version: number;
  updated_at: string;
  updated_by: string;
  date: string;
  gatekeeper: {
    run_permitted: boolean;
    hard_blocks: string[];
    caution_flags: string[];
    hours_since_last_run: number;
    rolling_7d_km: number;
    rolling_7d_runs: number;
    remaining_weekly_budget_km: number;
    effective_today_max_km: number;
  };
  plan_context: {
    source: string;
    revision: number;
    mesocycle_phase: string;
    walk_hr_cap_bpm: number;
  };
  shoe_eligibility: {
    eligible_run_shoes: string[];
    gated_shoes: string[];
    blocked_shoes: string[];
  };
  telemetry_provenance: {
    source: string;
    date: string;
  };
  prescription_summary: {
    decision: string;
    distance_km: number;
    duration_mins: number;
    shoe_platform: string;
    hr_ceiling_bpm: number;
  };
}

export type AthleteType = "runner" | "hyrox" | "hybrid_both";

export interface RunnerConfig {
  primary_distance: string | null;
  hr_first_governor: boolean;
  pace_floor_sec_per_km: number | null;
  z2_range_bpm: [number, number] | null;
  z3_range_bpm: [number, number] | null;
  lthr_bpm: number | null;
  vt1_bpm?: number | null;
  vt2_bpm?: number | null;
  vt1_pace_sec_per_km?: number | null;
  vt2_pace_sec_per_km?: number | null;
  max_hr_bpm?: number | null;
  resting_hr_bpm?: number | null;
}

export interface HyroxConfig {
  division: string | null;
  watch_logging_mode: string | null;
  target_total_time_sec: number | null;
  fresh_1km_pace_sec: number | null;
  compromised_1km_target_sec: number | null;
  weak_stations: string[];
}

export interface UserProfile {
  athlete_type: AthleteType;
  onboarding_completed: boolean;
  runner_config: RunnerConfig;
  hyrox_config: HyroxConfig;
}

export interface OrthopedicCheckIn {
  left_achilles: number;
  lumbar_sciatica: number;
  right_ankle: number;
  calves_soleus: number;
}

export interface PostRunContext {
  shoe_model: string;
  shoe_size: string;
  pre_run_weight_kg: number | null;
  post_run_weight_kg: number | null;
  fluid_consumed_ml: number | null;
  net_fluid_deficit_kg: number | null;
  net_sweat_rate_l_hr: number | null;
  orthopedic: OrthopedicCheckIn;
  pre_weight_manually_edited?: boolean;
  post_weight_manually_edited?: boolean;
  fluid_intake_confirmed?: boolean;
  weight_source?: string | null;
  sweat_status?: string | null;
  provisional_sweat_rate_l_hr?: number | null;
  final_sweat_rate_l_hr?: number | null;
  tempe_sensor_used?: boolean | null;
  garmin_notes?: string | null;
}

export interface ActivityRecord {
  garmin_id: number;
  name: string;
  type_key: string;
  start_iso: string;
  distance_m: number;
  duration_sec: number;
  moving_duration_sec: number;
  elevation_gain_m: number;
  average_hr: number | null;
  max_hr: number | null;
  cadence_spm: number | null;
  training_load: number | null;
  shoe_name: string | null;
  is_hyrox?: boolean | null;
  post_run_context?: PostRunContext | null;
  fit_temp_source?: string | null;
  garmin_notes?: string | null;
  is_pr?: boolean | null;
  start_latitude?: number | null;
  start_longitude?: number | null;
}

export type ThemeMode = "system" | "dark" | "light";

export interface BodyBatteryPoint {
  timestamp_ms: number;
  level: number;
}

export interface DayRecord {
  date: string;
  resting_hr: number | null;
  stress_avg: number | null;
  body_battery_high: number | null;
  body_battery_latest: number | null;
  body_battery_low: number | null;
  body_battery_charged: number | null;
  body_battery_drained: number | null;
  body_battery_series: BodyBatteryPoint[] | null;
  steps: number | null;
  step_goal: number | null;
  sleep_seconds: number | null;
  sleep_duration_sec: number | null;
  sleep_score: number | null;
  sleep_start: string | null;
  sleep_end: string | null;
  sleep_deep_sec: number | null;
  sleep_light_sec: number | null;
  sleep_rem_sec: number | null;
  sleep_awake_sec: number | null;
  hrv_last_night: number | null;
  hrv_status: string | null;
  hrv_weekly_avg: number | null;
  hrv_baseline_low: number | null;
  hrv_baseline_high: number | null;
  sleep_need_minutes?: number | null;
  sleep_baseline_minutes?: number | null;
  sleep_awake_count?: number | null;
  sleep_avg_hr?: number | null;
  sleep_stress?: number | null;
  sleep_respiration_avg?: number | null;
  sleep_respiration_low?: number | null;
  sleep_respiration_high?: number | null;
  sleep_quality?: string | null;
  hrv_last_night_5min_high?: number | null;
}

export interface DailyBriefingRecord {
  date: string;
  headline: string;
  body: string;
  action_cue: string;
  model_id: string;
  thinking_level: string;
  had_sleep: boolean;
  generated_at: string;
}

export interface PlanEnvelope {
  max_weekly_km: number;
  max_single_run_km: number;
  mesocycle_phase: string;
  min_hours_between_runs: number;
  walk_hr_cap_bpm: number | null;
}

export interface PlanFile {
  version: number;
  revision: number;
  updated_at: string;
  updated_by: string;
  covers: { from: string; to: string };
  status: string;
  envelope: PlanEnvelope;
  sessions: Array<{
    id: string;
    date: string;
    type: string;
    target: { duration_min: number[]; distance_km: number[] };
    hr_cap_bpm: number;
    shoe_constraints: string[];
    alternatives: string[];
  }>;
}

export interface BootstrapState {
  has_garmin_key: boolean;
  has_gemini_key: boolean;
  engine_mode: CoachEngineMode;
  shared_dir: string;
  settings: {
    custom_shared_coach_directory_path: string | null;
    execution_mode: ExecutionMode;
    profile_onboarded: boolean;
  };
  athlete_config: AthleteConfig;
  user_profile?: UserProfile;
  readiness_state: ReadinessState | null;
  activities: ActivityRecord[];
  days: DayRecord[];
  needs_initial_coach_setup?: boolean;
  model_settings?: ModelSettings;
  first_use_state?: FirstUseFlowState;
  daily_briefing?: DailyBriefingRecord | null;
  theme_mode?: ThemeMode;
  lifetime_backfill_completed?: boolean;
  total_activities_count?: number;
}

export interface CoachProfileThresholds {
  vt1_bpm?: number | null;
  vt2_bpm?: number | null;
  vt1_pace_sec_per_km?: number | null;
  vt2_pace_sec_per_km?: number | null;
  lthr_bpm?: number | null;
  max_hr_bpm?: number | null;
  resting_hr_bpm?: number | null;
  z2_range_bpm?: [number, number] | null;
  z3_range_bpm?: [number, number] | null;
  weight_kg?: number | null;
  primary_distance?: string | null;
  hr_first_governor?: boolean | null;
  hard_pace_floor_sec_per_km?: number | null;
  hyrox_division?: string | null;
  hyrox_watch_logging_mode?: string | null;
  hyrox_target_total_time_sec?: number | null;
  hyrox_fresh_1km_pace_sec?: number | null;
  hyrox_compromised_1km_target_sec?: number | null;
  hyrox_weak_stations?: string[] | null;
}

export interface CoachUpdateProposal {
  id: string;
  target_file: string;
  reason: string;
  updated_section_markdown: string;
  expires_on: string | null;
  diff_preview: string;
  status: string;
  plan_json?: unknown | null;
  profile_thresholds?: CoachProfileThresholds | null;
}

export interface FitTrackPoint {
  timestamp_sec: number;
  distance_m: number;
  speed_mps: number;
  pace_sec_per_km: number | null;
  heart_rate_bpm: number | null;
  cadence_spm: number | null;
  altitude_m: number | null;
  power_w?: number | null;
  gct_ms?: number | null;
  stance_time_pct?: number | null;
  vertical_oscillation_mm?: number | null;
  vertical_ratio_pct?: number | null;
  step_length_mm?: number | null;
  stance_balance_left_pct?: number | null;
  temperature_c?: number | null;
  respiration_brpm?: number | null;
  latitude_deg?: number | null;
  longitude_deg?: number | null;
  impact_load?: number | null;
}

export interface MetricDistribution {
  mean: number;
  min: number;
  max: number;
  first_half_mean: number | null;
  second_half_mean: number | null;
  drift: number | null;
}

export interface DirectionalHalfStats {
  sample_count: number;
  mean_hr: number | null;
  mean_pace_sec_per_km: number | null;
  mean_power_w: number | null;
  mean_cadence_spm: number | null;
  mean_gct_ms: number | null;
  mean_left_stance_balance_pct: number | null;
}

export interface DirectionalVectorSplit {
  turnaround_distance_m: number;
  turnaround_elapsed_s: number;
  max_displacement_m: number;
  outbound: DirectionalHalfStats;
  inbound: DirectionalHalfStats;
}

export interface VelocityBandStats {
  band: string;
  sample_count: number;
  mean_gct_ms: number | null;
  mean_cadence_spm: number | null;
  mean_vert_osc_mm: number | null;
  mean_vert_ratio_pct: number | null;
  mean_step_length_mm: number | null;
  mean_power_w: number | null;
  mean_left_stance_balance_pct: number | null;
}

export interface LapForensicDistribution {
  lap_index: number;
  distance_m: number;
  duration_s: number;
  mean_hr: number | null;
  mean_pace_sec_per_km: number | null;
  mean_power_w: number | null;
  mean_cadence_spm: number | null;
  mean_gct_ms: number | null;
  mean_vert_osc_mm: number | null;
  mean_stride_mm: number | null;
  mean_stance_balance_l_pct: number | null;
}

export interface ForensicMicroEvent {
  kind: string;
  elapsed_s: number;
  duration_s: number | null;
  distance_m: number | null;
  summary: string;
  pre_hr: number | null;
  post_hr: number | null;
  hr_bpm: number | null;
  pace_min_per_km: string | null;
  temp_c: number | null;
}

export interface ForensicTelemetryReport {
  sample_count: number;
  hr: MetricDistribution | null;
  pace_sec_per_km: MetricDistribution | null;
  power_w: MetricDistribution | null;
  cadence_spm: MetricDistribution | null;
  gct_ms: MetricDistribution | null;
  vertical_oscillation_mm: MetricDistribution | null;
  stride_length_mm: MetricDistribution | null;
  stance_balance_l_pct: MetricDistribution | null;
  left_stance_over_52_8_pct: number | null;
  respiration_brpm: MetricDistribution | null;
  impact_load_total: number | null;
  pw_hr_decoupling_pct: number | null;
  pa_hr_decoupling_pct: number | null;
  walk_seconds: number;
  run_seconds: number;
  walk_segments: number;
  run_segments: number;
  vt1_breach_seconds: number;
  vt2_breach_seconds: number;
  z5_breach_seconds: number;
  directional_vectors: DirectionalVectorSplit | null;
  velocity_bands: VelocityBandStats[];
  laps: LapForensicDistribution[];
  micro_events: ForensicMicroEvent[];
  temp_source?: string;
  garmin_notes?: string | null;
}

export interface FitSplit {
  km_index: number;
  distance_m: number;
  duration_sec: number;
  pace_sec_per_km: number | null;
  avg_hr_bpm: number | null;
  avg_cadence_spm: number | null;
  elevation_gain_m: number;
}

export interface FitGuardrailAudit {
  pace_floor_sec_per_km: number | null;
  easy_hr_ceiling_bpm: number | null;
  pace_floor_violations: number;
  hr_ceiling_violations: number;
  compliant: boolean;
}

export type LapKind = "run" | "station" | "roxzone";

export interface ClassifiedLap {
  lap_index: number;
  kind: LapKind;
  elapsed_sec: number;
  moving_sec: number;
  distance_m: number;
  pace_sec_per_km: number | null;
  avg_hr_bpm: number | null;
}

export interface HyroxDissection {
  is_hyrox_sim: boolean;
  run_splits: ClassifiedLap[];
  station_splits: ClassifiedLap[];
  roxzone_transitions: ClassifiedLap[];
  clean_run_pace_sec_per_km: number | null;
  clean_aerobic_efficiency: number | null;
  compromised_pace_decay_sec: number | null;
  station_hr_recovery_sec: number | null;
}

export interface FitTrackAnalysis {
  garmin_id: number;
  chart_points: FitTrackPoint[];
  splits: FitSplit[];
  hyrox_dissection?: HyroxDissection | null;
  forensic_telemetry?: ForensicTelemetryReport | null;
  temp_source?: string;
  decoupling_pct: number | null;
  audit: FitGuardrailAudit;
}

export interface FitBaselineProbe {
  scanned_files: number;
  detected_max_hr: number | null;
  detected_rhr_baseline: number | null;
  estimated_lthr: number | null;
  weekly_km_est: number | null;
}

export interface ActivityDetailPayload {
  activity: ActivityRecord;
  track: FitTrackAnalysis | null;
}

export interface NightHrvReading {
  timestamp_iso: string;
  timestamp_ms: number;
  value_ms: number;
}

export interface NightHrvDetail {
  date: string;
  readings: NightHrvReading[];
  last_night_avg: number | null;
  last_night_5min_high: number | null;
  baseline_low: number | null;
  baseline_high: number | null;
}

export interface DistanceRecordEntry {
  label: string;
  distance_m: number;
  time_sec: number;
  pace_sec_per_km: number;
  date: string;
  activity_id: number;
  activity_name: string;
  average_hr: number | null;
  source: string;
}

export interface MilestoneRecordEntry {
  key: string;
  label: string;
  value_display: string;
  value_num: number;
  date: string;
  activity_id: number | null;
  activity_name: string | null;
  average_hr: number | null;
  pace_sec_per_km: number | null;
}

export interface RacePredictionEntry {
  label: string;
  distance_m: number;
  predicted_sec: number;
  pace_sec_per_km: number;
  source_label: string;
  source_time_sec: number;
  source_date: string;
  source_activity_id: number;
}

export interface YearlyDistanceBest {
  year: number;
  bests: Record<string, DistanceRecordEntry>;
}

export interface ProgressionPoint {
  date: string;
  time_sec: number;
  pace_sec_per_km: number;
  activity_id: number;
  activity_name: string;
  is_record_break: boolean;
}

export interface DistanceProgressionSeries {
  label: string;
  distance_m: number;
  total_runs: number;
  broken_count: number;
  points: ProgressionPoint[];
}

export interface RecordsPayload {
  distance_records: DistanceRecordEntry[];
  milestones: MilestoneRecordEntry[];
  predictions: RacePredictionEntry[];
  prediction_window_months: number;
  yearly_bests: YearlyDistanceBest[];
  progression?: DistanceProgressionSeries[];
  pr_activities: ActivityRecord[];
  total_outdoor_runs: number;
  scanned_fit_runs: number;
}

export interface AnalyticsDatasetPayload {
  activities: ActivityRecord[];
  days: DayRecord[];
}

export interface CachedHealthTakes {
  sleep_take: DailyBriefingRecord | null;
  hrv_take: DailyBriefingRecord | null;
}

export type ExplorerSportFilter = "run" | "ride" | "all";

export interface ExplorerTally {
  count: number;
  distance_m: number;
}

export interface ExplorerCountryVisit {
  id: string;
  iso: string;
  name: string;
  continent: string;
  flag: string;
  label_lat: number;
  label_lon: number;
  first_iso: string;
  last_iso: string;
  count: number;
  distance_m: number;
}

export interface ExplorerCityVisit {
  city_id: number;
  name: string;
  country_id: string;
  country_iso: string;
  country_name: string;
  flag: string;
  lat: number;
  lon: number;
  first_iso: string;
  count: number;
  distance_m: number;
}

export interface ExplorerLeg {
  from_country_id: string;
  from_lat: number;
  from_lon: number;
  to_country_id: string;
  to_lat: number;
  to_lon: number;
  date_iso: string;
  year: number;
}

export interface ExplorerYearSummary {
  year: number;
  country_ids: string[];
  new_country_ids: string[];
  city_ids: number[];
  new_cities: number;
  country_tallies: Record<string, ExplorerTally>;
  city_tallies: Record<string, ExplorerTally>;
}

export interface ExplorerFirstVisit {
  country_id: string;
  iso: string;
  name: string;
  flag: string;
  first_iso: string;
}

export interface ExplorerSummaryPayload {
  sport_filter: ExplorerSportFilter | string;
  visits: ExplorerCountryVisit[];
  cities: ExplorerCityVisit[];
  legs: ExplorerLeg[];
  years: ExplorerYearSummary[];
  continents: string[];
  first_visit: ExplorerFirstVisit | null;
  total_located_activities: number;
  total_activities_in_filter: number;
}

export interface BackfillProgressEvent {
  phase: "fetching" | "saving" | "done" | string;
  page: number;
  fetched: number;
  message: string;
}

export interface RecordsProgressEvent {
  processed: number;
  total: number;
  message: string;
}

export function isHyroxDisciplineEnabled(
  profile: UserProfile | null | undefined,
  config: AthleteConfig | null | undefined
): boolean {
  if (profile?.athlete_type === "hyrox" || profile?.athlete_type === "hybrid_both") {
    return true;
  }
  if (config?.objective?.goalType === "hybrid_hyrox") {
    return true;
  }
  if (config?.discipline === "hyrox" || config?.discipline === "hybrid") {
    return true;
  }
  return false;
}
