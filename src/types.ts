export type ExecutionMode = "self_contained" | "external_server";
export type CoachEngineMode = "gemini_api" | "clipboard_packet";
export type GoalType =
  | "race"
  | "base_building"
  | "general_fitness"
  | "return_from_injury"
  | "hybrid_hyrox";

export interface HybridProfile {
  division: string | null;
  focusStations: string[];
  weeklyHybridSessionsTarget: number | null;
}

export interface AthleteConfig {
  specs: {
    displayName: string;
    weightKg: number | null;
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
}

export interface DayRecord {
  date: string;
  resting_hr: number | null;
  stress_avg: number | null;
  body_battery_high: number | null;
  body_battery_latest: number | null;
  steps: number | null;
  step_goal: number | null;
  sleep_seconds: number | null;
  sleep_score: number | null;
  hrv_last_night: number | null;
  hrv_status: string | null;
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
}

export interface CoachUpdateProposal {
  id: string;
  target_file: string;
  reason: string;
  updated_section_markdown: string;
  expires_on: string | null;
  diff_preview: string;
  status: string;
}

export interface FitTrackPoint {
  timestamp_sec: number;
  distance_m: number;
  speed_mps: number;
  pace_sec_per_km: number | null;
  heart_rate_bpm: number | null;
  cadence_spm: number | null;
  altitude_m: number | null;
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
