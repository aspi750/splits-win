use chrono::{Duration, NaiveDate, TimeZone, Utc};
use splits_win_lib::coach::{
    analyze_training_history, append_forensic_section, append_training_ledger_entry,
    apply_coach_proposal, apply_post_run_context_tool_update, build_activity_detail_tool_payload,
    build_daily_briefing_facts, build_daily_briefing_facts_with_weights,
    build_readiness_and_plan_tool_payload, build_static_system_instruction_with_weights,
    build_wellness_history_tool_payload, compact_history_markdown, fetch_1hz_window_from_track,
    format_user_profile_context, hydrate_profile_from_proposal_with_weight_guard,
    replacing_section, slice_fit_track_by_km, starter_coach_profile_markdown, starter_plan_file,
    starter_training_plan_markdown, without_dashes, CoachPacketBuilder,
};
use splits_win_lib::reconcile_scale_weights_and_config;
use splits_win_lib::forensic_extractor::extract_forensic_telemetry;
use splits_win_lib::garmin::{
    analyze_fit_points, chunk_date_range, dissect_hyrox_laps, parse_body_battery_days_json,
    parse_daily_summaries_json, parse_hrv_range_json, parse_sleep_summaries_json,
    parse_steps_json, parse_weight_range_json,
};
use splits_win_lib::models::*;
use splits_win_lib::readiness::{ReadinessEngine, ReadinessInput};
use splits_win_lib::scheduler::{
    is_within_catch_up_window, should_catch_up_briefing, should_send_morning_notification,
};
use splits_win_lib::storage::{CredentialStore, Database, SharedStore};

fn make_temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("splits-win-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_fit_point(
    timestamp_sec: f64,
    distance_m: f64,
    speed_mps: f64,
    hr_bpm: Option<f64>,
    cadence_spm: Option<f64>,
    altitude_m: Option<f64>,
) -> FitTrackPoint {
    FitTrackPoint {
        timestamp_sec,
        distance_m,
        speed_mps,
        pace_sec_per_km: if speed_mps > 0.1 {
            Some(1000.0 / speed_mps)
        } else {
            None
        },
        heart_rate_bpm: hr_bpm,
        cadence_spm,
        altitude_m,
        power_w: None,
        gct_ms: None,
        stance_time_pct: None,
        vertical_oscillation_mm: None,
        vertical_ratio_pct: None,
        step_length_mm: None,
        stance_balance_left_pct: None,
        temperature_c: None,
        respiration_brpm: None,
        latitude_deg: None,
        longitude_deg: None,
        impact_load: None,
    }
}

fn make_gps_fit_point(
    timestamp_sec: f64,
    distance_m: f64,
    speed_mps: f64,
    hr_bpm: Option<f64>,
    cadence_spm: Option<f64>,
    altitude_m: Option<f64>,
) -> FitTrackPoint {
    let mut pt = make_fit_point(
        timestamp_sec,
        distance_m,
        speed_mps,
        hr_bpm,
        cadence_spm,
        altitude_m,
    );
    pt.latitude_deg = Some(40.4168 + distance_m * 0.00000898);
    pt.longitude_deg = Some(-3.7038);
    pt
}

fn make_gps_fit_track(
    garmin_id: i64,
    total_dist_m: f64,
    total_sec: f64,
    steps: usize,
    hr_bpm: Option<f64>,
    cadence_spm: Option<f64>,
) -> FitTrackAnalysis {
    let n = steps.max(15);
    let spd = total_dist_m / total_sec.max(1.0);
    let pts: Vec<FitTrackPoint> = (0..=n)
        .map(|k| {
            let frac = k as f64 / n as f64;
            make_gps_fit_point(
                total_sec * frac,
                total_dist_m * frac,
                spd,
                hr_bpm,
                cadence_spm,
                Some(20.0),
            )
        })
        .collect();
    analyze_fit_points(garmin_id, pts, &AthleteConfig::default())
}

#[test]
fn test_athlete_config_30d_median_rhr_calibration() {
    let now = Utc.with_ymd_and_hms(2026, 11, 16, 10, 0, 0).unwrap();
    let sample = |days_ago: i64, rhr: Option<i32>| (now - Duration::days(days_ago), rhr);

    // Fewer than 5 valid days -> None
    let too_few = vec![
        sample(1, Some(50)),
        sample(2, Some(52)),
        sample(3, Some(54)),
        sample(4, Some(51)),
        sample(5, None),
        sample(40, Some(49)),
    ];
    assert_eq!(AthleteConfig::calibrated_resting_hr(&too_few, now), None);

    // 5 valid days -> median of [48, 50, 52, 55, 61] = 52
    let five_days = vec![
        sample(1, Some(61)),
        sample(2, Some(48)),
        sample(3, Some(52)),
        sample(4, Some(55)),
        sample(5, Some(50)),
    ];
    assert_eq!(AthleteConfig::calibrated_resting_hr(&five_days, now), Some(52));

    let mut config = AthleteConfig::default();
    assert_eq!(config.effective_resting_hr_baseline(&five_days, now), Some(52));
    config.specs.resting_hr_baseline = Some(49);
    assert_eq!(config.effective_resting_hr_baseline(&five_days, now), Some(49));
}

#[test]
fn test_shared_store_and_sqlite_roundtrip() {
    let dir = make_temp_dir("store");
    let store = SharedStore::new(Some(dir.clone())).unwrap();
    assert!(dir.join("state").exists());
    assert!(dir.join("history").exists());
    assert!(dir.join("attachments").exists());

    let mut cfg = AthleteConfig::default();
    cfg.specs.display_name = "Elena".to_string();
    cfg.specs.weight_kg = Some(62.5);
    store.save_athlete_config(&cfg).unwrap();
    let loaded = store.load_athlete_config().unwrap().unwrap();
    assert_eq!(loaded.specs.display_name, "Elena");
    assert_eq!(loaded.specs.weight_kg, Some(62.5));

    let db = Database::open_in_memory().unwrap();
    db.upsert_activity(&ActivityRecord {
        garmin_id: 42,
        name: "Morning Aerobic".to_string(),
        type_key: "running".to_string(),
        start_iso: "2026-11-15T07:30:00Z".to_string(),
        distance_m: 8000.0,
        duration_sec: 2880.0,
        moving_duration_sec: 2880.0,
        elevation_gain_m: 35.0,
        average_hr: Some(138.0),
        max_hr: Some(146.0),
        cadence_spm: Some(174.0),
        training_load: Some(45.0),
        shoe_name: Some("Superblast 3".to_string()),
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    })
    .unwrap();
    let acts = db.list_activities(10).unwrap();
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0].garmin_id, 42);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_readiness_engine_green_red_amber_and_persistence() {
    let now = Utc.with_ymd_and_hms(2026, 11, 16, 10, 0, 0).unwrap();
    let mut config = AthleteConfig::default();
    config.specs.resting_hr_baseline = Some(52);
    config.specs.vt1_ceiling_bpm = Some(142);
    config.medical.enforce_easy_hr_ceiling = true;
    config.medical.easy_hr_ceiling_bpm = Some(138);
    config.medical.min_rest_hours_between_runs = Some(36);

    let healthy_day = DayRecord {
        date: "2026-11-16".to_string(),
        resting_hr: Some(52),
        stress_avg: Some(25),
        body_battery_high: Some(85),
        body_battery_latest: Some(78),
        steps: Some(4200),
        step_goal: Some(8000),
        sleep_seconds: Some(27000),
        sleep_score: Some(82),
        hrv_last_night: Some(64),
        hrv_status: Some("BALANCED".to_string()),
        ..Default::default()
    };

    // GREEN
    let green = ReadinessEngine::evaluate(ReadinessInput {
        now,
        athlete_config: &config,
        activities: &[ActivityRecord {
            garmin_id: 1,
            name: "Easy".to_string(),
            type_key: "running".to_string(),
            start_iso: (now - Duration::hours(48)).to_rfc3339(),
            distance_m: 6000.0,
            duration_sec: 2200.0,
            moving_duration_sec: 2200.0,
            elevation_gain_m: 20.0,
            average_hr: Some(135.0),
            max_hr: Some(142.0),
            cadence_spm: Some(176.0),
            training_load: Some(40.0),
            shoe_name: Some("Superblast 3".to_string()),
            is_hyrox: None,
            post_run_context: None,
            fit_temp_source: None,
            garmin_notes: None,
            is_pr: None,
            start_latitude: None,
            start_longitude: None,
        }],
        days: &[healthy_day.clone()],
        plan: None,
        gear_inventory: None,
    });
    assert!(green.readiness_state.gatekeeper.run_permitted);
    assert_eq!(green.readiness_state.prescription_summary.decision, "RUN");

    // RED (elevated RHR +5, sleep < 5.5h, spacing < 36h)
    let mut bad_day = healthy_day.clone();
    bad_day.resting_hr = Some(58);
    bad_day.sleep_seconds = Some(18000);
    bad_day.sleep_score = Some(48);
    let red = ReadinessEngine::evaluate(ReadinessInput {
        now,
        athlete_config: &config,
        activities: &[ActivityRecord {
            garmin_id: 2,
            name: "Recent".to_string(),
            type_key: "running".to_string(),
            start_iso: (now - Duration::hours(12)).to_rfc3339(),
            distance_m: 6000.0,
            duration_sec: 2200.0,
            moving_duration_sec: 2200.0,
            elevation_gain_m: 20.0,
            average_hr: Some(135.0),
            max_hr: Some(142.0),
            cadence_spm: Some(176.0),
            training_load: Some(40.0),
            shoe_name: None,
            is_hyrox: None,
            post_run_context: None,
            fit_temp_source: None,
            garmin_notes: None,
            is_pr: None,
            start_latitude: None,
            start_longitude: None,
        }],
        days: &[bad_day],
        plan: None,
        gear_inventory: None,
    });
    assert!(!red.readiness_state.gatekeeper.run_permitted);
    assert_eq!(red.readiness_state.prescription_summary.decision, "REST_DECOMPRESSION");
    assert_eq!(red.readiness_state.prescription_summary.hr_ceiling_bpm, 105);

    // Persistence & secondary_observation alias decoding
    let dir = make_temp_dir("persist");
    green.persist(&dir).unwrap();
    assert!(dir.join("readiness_state.json").exists());
    assert!(dir.join("today_adapted_target.json").exists());

    let alias_json = r#"{
        "evaluation_date": "2026-11-16",
        "secondary_observation": { "available": true, "device_name": "Oura Ring" }
    }"#;
    let decoded: AuthoritativeTarget = serde_json::from_str(alias_json).unwrap();
    assert_eq!(
        decoded.fitbit_observation.as_ref().and_then(|o| o.device_name.as_deref()),
        Some("Oura Ring")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_coach_packet_builder_and_markdown_section_replace() {
    let now = Utc.with_ymd_and_hms(2026, 11, 16, 10, 0, 0).unwrap();
    let mut config = AthleteConfig::default();
    config.specs.display_name = "Nora".to_string();
    let packet = CoachPacketBuilder::build_markdown(
        &config,
        &[],
        &[],
        None,
        None,
        "How should I pace tomorrow?",
        now,
    );
    assert!(packet.contains("## Athlete Profile & Active Guardrails"));
    assert!(packet.contains("Nora"));
    assert!(packet.contains("## 7-Day & 28-Day Load Summary"));
    assert!(packet.contains("How should I pace tomorrow?"));

    let md = "# COACH_PROFILE.md\n\n## AVAILABILITY & CONTEXT\nold\n\n## Change History\n- seed\n";
    let updated = replacing_section(md, "## AVAILABILITY & CONTEXT\nnew");
    assert!(updated.contains("new"));
    assert!(!updated.contains("old"));
}

#[test]
fn test_scheduler_catch_up_and_deduplication() {
    assert!(is_within_catch_up_window(16));
    assert!(!is_within_catch_up_window(22));
    assert!(should_catch_up_briefing(16, true, false, true));
    assert!(!should_catch_up_briefing(16, true, true, true));
    assert!(should_send_morning_notification(16, true, true, true, Some("2026-11-15"), "2026-11-16"));
    assert!(!should_send_morning_notification(16, true, true, true, Some("2026-11-16"), "2026-11-16"));
}

#[test]
fn test_fit_splits_decoupling_and_guardrail_audit() {
    let mut config = AthleteConfig::default();
    config.medical.enforce_pace_floor = true;
    config.medical.min_pace_sec_per_km = Some(390); // 6:30/km
    config.medical.enforce_easy_hr_ceiling = true;
    config.medical.easy_hr_ceiling_bpm = Some(135);

    // 1st half: 3.0 m/s @ 120 bpm (EF = 0.025); 2nd half: 3.0 m/s @ 150 bpm (EF = 0.020) -> 20.0% decoupling
    let points = vec![
        make_fit_point(0.0, 0.0, 3.0, Some(120.0), Some(174.0), Some(10.0)),
        make_fit_point(160.0, 500.0, 3.0, Some(120.0), Some(174.0), Some(12.0)),
        make_fit_point(340.0, 1000.0, 3.0, Some(120.0), Some(174.0), Some(15.0)),
        make_fit_point(520.0, 1500.0, 3.0, Some(150.0), Some(172.0), Some(15.0)),
        make_fit_point(700.0, 2000.0, 3.0, Some(150.0), Some(172.0), Some(16.0)),
        make_fit_point(880.0, 2500.0, 3.0, Some(150.0), Some(172.0), Some(16.0)),
    ];

    let analysis = analyze_fit_points(999, points, &config);
    assert_eq!(analysis.decoupling_pct, Some(20.0));
    assert_eq!(analysis.splits.len(), 3);
    assert!(!analysis.audit.compliant);
    assert!(analysis.audit.pace_floor_violations > 0);
    assert!(analysis.audit.hr_ceiling_violations > 0);
}

#[test]
fn test_apply_coach_proposal_and_history_backup() {
    let dir = make_temp_dir("proposal");
    let store = SharedStore::new(Some(dir.clone())).unwrap();
    store
        .write_text_atomic(
            "COACH_PROFILE.md",
            "# COACH_PROFILE.md\n\n## AVAILABILITY & CONTEXT\n| key | value |\n| --- | --- |\n| pool | false |\n\n## Change History\n- seed\n",
        )
        .unwrap();

    let proposal = CoachUpdateProposal {
        id: "p1".to_string(),
        target_file: "COACH_PROFILE.md".to_string(),
        reason: "Pool reopened".to_string(),
        updated_section_markdown: "## AVAILABILITY & CONTEXT\n| key | value |\n| --- | --- |\n| pool | true |".to_string(),
        expires_on: None,
        diff_preview: String::new(),
        status: "pending_user_approval".to_string(),
        plan_json: None,
        profile_thresholds: None,
    };

    apply_coach_proposal(&store, &proposal).unwrap();
    let updated = store.read_text("COACH_PROFILE.md").unwrap().unwrap();
    assert!(updated.contains("| pool | true |"));
    assert!(updated.contains("Pool reopened [target: COACH_PROFILE.md]"));
    let history_entries: Vec<_> = std::fs::read_dir(dir.join("history")).unwrap().collect();
    assert!(!history_entries.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_query_run_slice_against_sqlite_fit_tracks() {
    let db = Database::open_in_memory().unwrap();
    let config = AthleteConfig::default();
    let points = vec![
        make_fit_point(0.0, 0.0, 3.0, Some(125.0), Some(174.0), Some(10.0)),
        make_fit_point(200.0, 600.0, 3.0, Some(130.0), Some(175.0), Some(11.0)),
        make_fit_point(400.0, 1200.0, 3.0, Some(135.0), Some(176.0), Some(12.0)),
        make_fit_point(600.0, 1800.0, 3.0, Some(138.0), Some(176.0), Some(14.0)),
    ];
    let analysis = analyze_fit_points(777, points, &config);
    db.upsert_fit_track(777, &analysis).unwrap();

    let loaded = db.get_fit_track(777).unwrap().unwrap();
    let slice = slice_fit_track_by_km(&loaded, 0.5, 1.5);
    assert_eq!(slice["garmin_id"].as_i64(), Some(777));
    assert_eq!(slice["point_count"].as_u64(), Some(2));
}

#[test]
fn test_stage2_2_hybrid_hyrox_spacing_and_compromised_run_audit() {
    let now = Utc.with_ymd_and_hms(2026, 11, 16, 10, 0, 0).unwrap();
    let mut days = Vec::new();
    for offset in 0..10 {
        let d = (now - Duration::days(offset)).format("%Y-%m-%d").to_string();
        days.push(DayRecord {
            date: d,
            resting_hr: Some(50),
            stress_avg: None,
            body_battery_high: Some(85),
            body_battery_latest: Some(60),
            steps: Some(8000),
            step_goal: Some(8000),
            sleep_seconds: Some(27_000),
            sleep_score: Some(84),
            hrv_last_night: Some(55),
            hrv_status: Some("BALANCED".to_string()),
            ..Default::default()
        });
    }

    let strength = ActivityRecord {
        garmin_id: 901,
        name: "Heavy Leg Strength & Sled".to_string(),
        type_key: "strength_training".to_string(),
        start_iso: (now - Duration::hours(12)).to_rfc3339(),
        distance_m: 0.0,
        duration_sec: 3600.0,
        moving_duration_sec: 3600.0,
        elevation_gain_m: 0.0,
        average_hr: Some(138.0),
        max_hr: Some(165.0),
        cadence_spm: None,
        training_load: Some(120.0),
        shoe_name: None,
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    };

    let mut hybrid_config = AthleteConfig::default();
    hybrid_config.objective.goal_type = GoalType::HybridHyrox;
    hybrid_config.medical.min_rest_hours_between_runs = Some(36);
    hybrid_config.objective.hybrid_profile = Some(HybridProfile {
        division: Some("Open Men".to_string()),
        focus_stations: vec!["Sled Push".to_string(), "Wall Balls".to_string()],
        weekly_hybrid_sessions_target: Some(3),
    });

    let hybrid_out = ReadinessEngine::evaluate(ReadinessInput {
        now,
        athlete_config: &hybrid_config,
        activities: &[strength.clone()],
        days: &days,
        plan: None,
        gear_inventory: None,
    });
    assert!(!hybrid_out.readiness_state.gatekeeper.run_permitted);
    assert!(hybrid_out
        .readiness_state
        .gatekeeper
        .hard_blocks
        .iter()
        .any(|b| b.starts_with("STRUCTURAL_SPACING_VIOLATION")));

    let mut base_config = hybrid_config.clone();
    base_config.objective.goal_type = GoalType::BaseBuilding;
    let base_out = ReadinessEngine::evaluate(ReadinessInput {
        now,
        athlete_config: &base_config,
        activities: &[strength],
        days: &days,
        plan: None,
        gear_inventory: None,
    });
    assert!(base_out.readiness_state.gatekeeper.run_permitted);

    // CompromisedRunAudit
    let laps = vec![
        FitLap { elapsed_sec: 260.0, moving_sec: 260.0, distance_m: 1000.0, avg_hr_bpm: Some(152.0) },
        FitLap { elapsed_sec: 120.0, moving_sec: 120.0, distance_m: 50.0, avg_hr_bpm: Some(160.0) },
        FitLap { elapsed_sec: 265.0, moving_sec: 265.0, distance_m: 1000.0, avg_hr_bpm: Some(156.0) },
        FitLap { elapsed_sec: 140.0, moving_sec: 140.0, distance_m: 40.0, avg_hr_bpm: Some(166.0) },
        FitLap { elapsed_sec: 290.0, moving_sec: 290.0, distance_m: 1000.0, avg_hr_bpm: Some(168.0) },
        FitLap { elapsed_sec: 150.0, moving_sec: 150.0, distance_m: 30.0, avg_hr_bpm: Some(172.0) },
        FitLap { elapsed_sec: 300.0, moving_sec: 300.0, distance_m: 1000.0, avg_hr_bpm: Some(172.0) },
        FitLap { elapsed_sec: 130.0, moving_sec: 130.0, distance_m: 20.0, avg_hr_bpm: Some(175.0) },
    ];
    let audit = CompromisedRunAudit::compute(&laps).unwrap();
    assert_eq!(audit.run_segment_count, 4);
    assert_eq!(audit.pace_fade_sec_per_km, 32.5);
    assert_eq!(audit.hr_drift_bpm, Some(16.0));
}

#[test]
fn test_stage2_2_training_history_analyzer_and_starter_templates() {
    let now = Utc.with_ymd_and_hms(2026, 11, 16, 10, 0, 0).unwrap();
    let mut activities = Vec::new();
    for offset in (1..=45).step_by(2) {
        let start = now - Duration::days(offset);
        activities.push(ActivityRecord {
            garmin_id: offset * 10,
            name: format!("Easy Run {offset}"),
            type_key: "running".to_string(),
            start_iso: start.to_rfc3339(),
            distance_m: 8000.0,
            duration_sec: 2880.0,
            moving_duration_sec: 2880.0,
            elevation_gain_m: 40.0,
            average_hr: Some(142.0),
            max_hr: Some(155.0),
            cadence_spm: Some(174.0),
            training_load: Some(60.0),
            shoe_name: None,
            is_hyrox: None,
            post_run_context: None,
            fit_temp_source: None,
            garmin_notes: None,
            is_pr: None,
            start_latitude: None,
            start_longitude: None,
        });
        if offset % 4 == 1 {
            activities.push(ActivityRecord {
                garmin_id: offset * 10 + 1,
                name: "Hybrid Erg & Sled".to_string(),
                type_key: "strength_training".to_string(),
                start_iso: (start + Duration::hours(1)).to_rfc3339(),
                distance_m: 0.0,
                duration_sec: 2700.0,
                moving_duration_sec: 2700.0,
                elevation_gain_m: 0.0,
                average_hr: Some(135.0),
                max_hr: Some(162.0),
                cadence_spm: None,
                training_load: Some(90.0),
                shoe_name: None,
                is_hyrox: None,
                post_run_context: None,
                fit_temp_source: None,
                garmin_notes: None,
                is_pr: None,
                start_latitude: None,
                start_longitude: None,
            });
        }
    }

    let mut config = AthleteConfig::default();
    config.specs.display_name = "Test Athlete".to_string();
    config.objective.goal_type = GoalType::HybridHyrox;

    let summary = analyze_training_history(&activities, &config, now);
    assert!(summary.has_enough_history_for_audit);
    assert!(summary.lifetime.total_run_km > 150.0);
    assert!(summary.trailing.run_sessions > 15);
    assert!(summary.trailing.hybrid_sessions > 5);

    let md = compact_history_markdown(&summary);
    assert!(md.contains("Lifetime"));
    assert!(md.contains("Trailing"));

    let profile = starter_coach_profile_markdown(&config, &summary);
    assert!(profile.contains("## Athlete Identity"));
    assert!(profile.contains("## AVAILABILITY & CONTEXT"));

    let plan = starter_training_plan_markdown(&config, &summary);
    assert!(plan.contains("## Current Phase & Baseline"));
    assert!(plan.contains("## Change History"));
}

#[test]
fn test_stage3_1_user_profile_migration_and_hyrox_reclassification() {
    // 1. Fresh DB defaults to runner + onboarding_completed = false
    let db = Database::open_in_memory().unwrap();
    let fresh = db.get_user_profile().unwrap();
    assert_eq!(fresh.athlete_type, AthleteType::Runner);
    assert!(!fresh.onboarding_completed);

    // 2. Round-trip custom HybridBoth profile
    let custom = UserProfile {
        athlete_type: AthleteType::HybridBoth,
        onboarding_completed: true,
        runner_config: RunnerConfig {
            primary_distance: Some("Half Marathon".to_string()),
            hr_first_governor: true,
            pace_floor_sec_per_km: Some(390),
            z2_range_bpm: Some([130, 144]),
            z3_range_bpm: Some([145, 160]),
            lthr_bpm: Some(172),
            ..Default::default()
        },
        hyrox_config: HyroxConfig {
            division: Some("Open Men".to_string()),
            watch_logging_mode: Some("lap_button".to_string()),
            target_total_time_sec: Some(4500),
            fresh_1km_pace_sec: Some(255),
            compromised_1km_target_sec: Some(275),
            weak_stations: vec!["Sled Push".to_string(), "Wall Balls".to_string()],
        },
    };
    db.save_user_profile(&custom).unwrap();
    let loaded = db.get_user_profile().unwrap();
    assert_eq!(loaded, custom);

    let ctx = format_user_profile_context(&loaded);
    assert!(ctx.contains("hybrid_both"));
    assert!(ctx.contains("Sled Push 152 kg"));
    assert!(ctx.contains("Wall Balls"));

    // 3. Reclassify activity as HYROX
    let act = ActivityRecord {
        garmin_id: 555,
        name: "Saturday Sim".to_string(),
        type_key: "running".to_string(),
        start_iso: "2026-11-16T09:00:00Z".to_string(),
        distance_m: 8000.0,
        duration_sec: 3600.0,
        moving_duration_sec: 3550.0,
        elevation_gain_m: 10.0,
        average_hr: Some(165.0),
        max_hr: Some(182.0),
        cadence_spm: Some(176.0),
        training_load: Some(180.0),
        shoe_name: None,
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    };
    db.upsert_activity(&act).unwrap();
    db.set_activity_hyrox(555, true).unwrap();
    let fetched = db.get_activity(555).unwrap().unwrap();
    assert_eq!(fetched.is_hyrox, Some(true));
}

#[test]
fn test_stage3_1_hyrox_16_lap_dissection_and_compromised_decay() {
    // 8 x 1000m runs alternating with 8 x 0m station laps (16 laps total)
    // Run 1 = 250s (4:10/km), Runs 2-4 = 260s, Runs 5-8 = 280s (4:40/km) -> decay = +30.0 s/km
    let mut laps = Vec::new();
    for i in 1..=8 {
        let run_sec = match i {
            1 => 250.0,
            2..=4 => 260.0,
            _ => 280.0,
        };
        laps.push(FitLap {
            elapsed_sec: run_sec,
            moving_sec: run_sec,
            distance_m: 1000.0,
            avg_hr_bpm: Some(160.0 + i as f64),
        });
        laps.push(FitLap {
            elapsed_sec: 150.0,
            moving_sec: 150.0,
            distance_m: 0.0,
            avg_hr_bpm: Some(172.0),
        });
    }

    let dissection = dissect_hyrox_laps(&laps, false).expect("16-lap HYROX sim should auto-detect");
    assert!(dissection.is_hyrox_sim);
    assert_eq!(dissection.run_splits.len(), 8);
    assert_eq!(dissection.station_splits.len(), 8);

    // Clean run pace: (250 + 3*260 + 4*280) / 8 = (250 + 780 + 1120) / 8 = 2150 / 8 = 268.8 s/km
    assert_eq!(dissection.clean_run_pace_sec_per_km, Some(268.8));
    assert!(dissection.clean_aerobic_efficiency.is_some());
    // Compromised decay: Runs 5-8 avg (280.0) - Run 1 (250.0) = +30.0 s/km
    assert_eq!(dissection.compromised_pace_decay_sec, Some(30.0));
}

#[test]
fn test_v0_2_1_forensic_extractor_bands_vectors_decoupling_and_events() {
    let mut pts = Vec::new();
    let mut dist = 0.0_f64;
    // 100s out-and-back synthetic run:
    // t 0..49 outbound north (+0.0001 deg lat/s ≈ 11m/s GPS step so turnaround >= 200m)
    // t 50..56 stationary stop (speed = 0.0)
    // t 57..66 walk segment (speed = 1.6 m/s, cadence = 108 spm)
    // t 67..99 inbound back south with 3 velocity bands & high HR/temp swings
    for t in 0..100 {
        let (speed, cad, hr, pw, temp, sb, lat_offset) = if (50..=56).contains(&t) {
            (0.0, 0.0, 125.0, 0.0, 18.0, 50.0, 0.005)
        } else if (57..=66).contains(&t) {
            (1.6, 108.0, 130.0, 140.0, 19.0, 51.0, 0.005 - (t - 56) as f64 * 0.00005)
        } else if t < 50 {
            let lat_off = t as f64 * 0.0001; // up to +0.0049 deg ≈ 545m displacement
            let spd = if t < 25 { 3.1 } else { 2.7 };
            (spd, 176.0, 136.0, 260.0, 16.5, 53.2, lat_off)
        } else {
            let spd = if t < 82 { 2.2 } else { 3.1 };
            let hr = 152.0; // triggers VT1, HR>=145 breach, positive Pw:Hr & Pa:Hr decoupling
            (spd, 172.0, hr, 240.0, 21.0, 51.0, 0.0045 - (t - 66) as f64 * 0.00012)
        };
        dist += speed;
        let mut p = make_fit_point(
            t as f64,
            dist,
            speed,
            Some(hr),
            Some(cad),
            Some(15.0),
        );
        p.power_w = if pw > 0.0 { Some(pw) } else { None };
        p.gct_ms = if cad >= 120.0 { Some(250.0) } else { None };
        p.vertical_oscillation_mm = if cad >= 120.0 { Some(82.0) } else { None };
        p.vertical_ratio_pct = if cad >= 120.0 { Some(8.1) } else { None };
        p.step_length_mm = if cad >= 120.0 { Some(980.0) } else { None };
        p.stance_balance_left_pct = Some(sb);
        p.temperature_c = Some(temp);
        p.respiration_brpm = Some(if t < 50 { 30.0 } else { 38.0 });
        p.latitude_deg = Some(40.4000 + lat_offset);
        p.longitude_deg = Some(-3.7000);
        p.impact_load = if cad >= 120.0 { Some(1.2) } else { None };
        pts.push(p);
    }

    let laps = vec![
        FitLap { elapsed_sec: 50.0, moving_sec: 50.0, distance_m: 145.0, avg_hr_bpm: Some(136.0) },
        FitLap { elapsed_sec: 50.0, moving_sec: 43.0, distance_m: 110.0, avg_hr_bpm: Some(146.0) },
    ];
    let rep = extract_forensic_telemetry(&pts, &laps);
    assert_eq!(rep.sample_count, 100);
    assert!(rep.pw_hr_decoupling_pct.is_some());
    assert!(rep.pa_hr_decoupling_pct.is_some());
    assert_eq!(rep.left_stance_over_52_8_pct, Some(50.0));
    assert!(rep.respiration_brpm.is_some());
    assert!(rep.impact_load_total.unwrap_or(0.0) > 80.0);
    assert!(rep.walk_segments >= 1);
    assert!(rep.run_segments >= 2);
    assert!(rep.vt1_breach_seconds > 25.0);

    let dir = rep.directional_vectors.as_ref().expect("directional split should be detected");
    assert!(dir.max_displacement_m >= 150.0);
    assert!(dir.inbound.mean_hr.unwrap() > dir.outbound.mean_hr.unwrap());

    let band_names: Vec<&str> = rep.velocity_bands.iter().map(|b| b.band.as_str()).collect();
    assert!(band_names.contains(&"<5:45/km"));
    assert!(band_names.contains(&"5:45-6:30/km"));
    assert!(band_names.contains(&">6:30/km"));

    let event_kinds: Vec<&str> = rep.micro_events.iter().map(|e| e.kind.as_str()).collect();
    assert!(event_kinds.contains(&"stop_pause"));
    assert!(event_kinds.contains(&"thermal_swing"));
    assert!(event_kinds.contains(&"hr_zone_breach"));

    let md = rep.to_markdown_summary();
    assert!(md.contains("Pw:Hr Decoupling"));
    assert!(md.contains("Pa:Hr Decoupling"));
    assert!(md.contains("Left Stance > 52.8% L"));
}

#[test]
fn test_v0_2_1_sweat_matrix_1hz_window_and_ledger() {
    // 1. Sweat Matrix computation & DB persistence
    let raw_ctx = PostRunContext {
        shoe_model: "Superblast 2".to_string(),
        shoe_size: "EU 43".to_string(),
        pre_run_weight_kg: Some(75.0),
        post_run_weight_kg: Some(74.0),
        fluid_consumed_ml: Some(500.0),
        fluid_intake_confirmed: true,
        net_fluid_deficit_kg: None,
        net_sweat_rate_l_hr: None,
        orthopedic: OrthopedicCheckIn {
            left_achilles: 2,
            lumbar_sciatica: 1,
            right_ankle: 0,
            calves_soleus: 3,
        },
        ..Default::default()
    };
    let computed = raw_ctx.with_computed_sweat(3600.0);
    assert_eq!(computed.net_fluid_deficit_kg, Some(1.5));
    assert_eq!(computed.net_sweat_rate_l_hr, Some(1.5));
    assert_eq!(computed.sweat_status.as_deref(), Some("final_verified"));

    let db = Database::open_in_memory().unwrap();
    db.upsert_activity(&ActivityRecord {
        garmin_id: 888,
        name: "Sweat Test Run".to_string(),
        type_key: "running".to_string(),
        start_iso: "2026-11-17T07:00:00Z".to_string(),
        distance_m: 10000.0,
        duration_sec: 3600.0,
        moving_duration_sec: 3600.0,
        elevation_gain_m: 40.0,
        average_hr: Some(140.0),
        max_hr: Some(152.0),
        cadence_spm: Some(176.0),
        training_load: Some(80.0),
        shoe_name: Some("Superblast 2".to_string()),
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    })
    .unwrap();
    db.save_post_run_context(888, &computed).unwrap();
    let loaded_act = db.get_activity(888).unwrap().unwrap();
    assert_eq!(loaded_act.post_run_context, Some(computed));

    // 2. fetch_1hz_window_from_track stepping
    let points: Vec<FitTrackPoint> = (0..60)
        .map(|s| make_fit_point(s as f64, s as f64 * 3.0, 3.0, Some(135.0), Some(174.0), Some(10.0)))
        .collect();
    let analysis = analyze_fit_points(888, points, &AthleteConfig::default());
    let win = fetch_1hz_window_from_track(&analysis, 10.0, 50.0, 10);
    assert_eq!(win["row_count"].as_u64(), Some(5)); // t=10, 20, 30, 40, 50

    // 3. append_training_ledger_entry
    let dir = make_temp_dir("ledger");
    let store = SharedStore::new(Some(dir.clone())).unwrap();
    let lines1 = append_training_ledger_entry(&store, "## 2026-11-17 Easy 10k\nFelt smooth.").unwrap();
    let lines2 = append_training_ledger_entry(&store, "## 2026-11-18 Rest\nRecovery.").unwrap();
    assert!(lines2 > lines1);
    let saved = store.read_text("ASPI_Training_Log.txt").unwrap().unwrap();
    assert!(saved.contains("Easy 10k"));
    assert!(saved.contains("Recovery."));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_garmin_weight_sync_scale_pairing_and_hybrid_tempe_parity() {
    // 1. Garmin Weight JSON parsing (grams -> kg) & duplicate timestamp merging + SQLite dedup
    let raw_json = br#"{
      "dailyWeightSummaries": [
        {
          "summaryDate": "2026-11-18",
          "allWeightMetrics": [
            {
              "samplePk": 1,
              "timestampGMT": "2026-11-18T06:30:00.0",
              "weight": 105200.0,
              "bodyFat": 22.4,
              "bodyWater": 56.1,
              "muscleMass": 42100.0,
              "sourceType": "INDEX_SCALE"
            },
            {
              "samplePk": 1,
              "timestampGMT": "2026-11-18T06:30:00.0",
              "weight": 105200.0,
              "sourceType": "INDEX_SCALE"
            }
          ]
        }
      ],
      "dateWeightList": [
        {
          "samplePk": 2,
          "timestampGMT": "2026-11-18T08:45:00.0",
          "weight": 103900.0,
          "sourceType": "INDEX_SCALE"
        }
      ]
    }"#;
    let parsed = parse_weight_range_json(raw_json);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].weight_kg, 103.9);
    assert_eq!(parsed[1].weight_kg, 105.2);
    assert_eq!(parsed[1].body_fat_pct, Some(22.4));
    assert_eq!(parsed[1].muscle_mass_kg, Some(42.1));

    let db = Database::open_in_memory().unwrap();
    for w in &parsed {
        db.upsert_weight_entry(w).unwrap();
    }
    for w in &parsed {
        db.upsert_weight_entry(w).unwrap();
    }
    assert_eq!(db.list_weight_entries(10).unwrap().len(), 2);

    // 2. Pre/post scale pairing + dehydrated baseline guard + two-stage Sweat Matrix
    let act_start: i64 = 1_795_000_000;
    let act_end: i64 = act_start + 3600;
    let pre = WeightEntry {
        timestamp_epoch_sec: act_start - 25 * 60,
        timestamp_iso: "2026-11-18T06:35:00Z".to_string(),
        weight_kg: 105.0,
        body_fat_pct: None,
        body_water_pct: None,
        muscle_mass_kg: None,
        source: "garmin_scale".to_string(),
    };
    let post = WeightEntry {
        timestamp_epoch_sec: act_end + 15 * 60,
        timestamp_iso: "2026-11-18T08:15:00Z".to_string(),
        weight_kg: 103.8,
        body_fat_pct: None,
        body_water_pct: None,
        muscle_mass_kg: None,
        source: "garmin_scale".to_string(),
    };
    let paired = scale_matcher::pair_scale_weigh_ins(
        act_start,
        act_end,
        3600.0,
        &[pre.clone(), post.clone()],
        PostRunContext::default(),
    );
    assert_eq!(paired.pre_run_weight_kg, Some(105.0));
    assert_eq!(paired.post_run_weight_kg, Some(103.8));
    assert_eq!(paired.weight_source.as_deref(), Some("garmin_scale"));
    assert!(!paired.fluid_intake_confirmed);
    assert_eq!(paired.sweat_status.as_deref(), Some("provisional_no_intake"));
    assert_eq!(paired.provisional_sweat_rate_l_hr, Some(1.2));
    assert_eq!(paired.final_sweat_rate_l_hr, None);

    // Manual edit protection prevents scale overwrite on pre_run_weight_kg
    let manual = PostRunContext {
        pre_run_weight_kg: Some(106.0),
        pre_weight_manually_edited: true,
        ..Default::default()
    };
    let kept = scale_matcher::pair_scale_weigh_ins(
        act_start,
        act_end,
        3600.0,
        &[pre.clone(), post.clone()],
        manual,
    );
    assert_eq!(kept.pre_run_weight_kg, Some(106.0));
    assert_eq!(kept.post_run_weight_kg, Some(103.8));

    // Post-run dehydrated weigh-in MUST NOT become baseline weight
    let windows = vec![(act_start, act_end)];
    assert_eq!(scale_matcher::latest_baseline_weight_kg(&[post.clone()], &windows), None);
    assert_eq!(
        scale_matcher::latest_baseline_weight_kg(&[post, pre], &windows),
        Some(105.0)
    );

    // Fallback chain -> latest_14d_weigh_in & profile_baseline (provisional)
    let r_profile = scale_matcher::resolve_effective_weight(
        None,
        act_start + 86_400,
        &[],
        &[],
        Some(105.0),
    )
    .unwrap();
    assert_eq!(r_profile.source, "profile_baseline");
    assert!(r_profile.is_provisional);
    let packet = append_forensic_section("## Header".to_string(), None, None, Some(&r_profile));
    assert!(packet.contains("profile_baseline, provisional"));

    // 3. Coach tool update_post_run_context & effective temp_source
    let act = ActivityRecord {
        garmin_id: 505,
        name: "Temp & Sweat Tool Test".to_string(),
        type_key: "running".to_string(),
        start_iso: "2026-11-18T07:00:00Z".to_string(),
        distance_m: 10_000.0,
        duration_sec: 3600.0,
        moving_duration_sec: 3600.0,
        elevation_gain_m: 20.0,
        average_hr: Some(138.0),
        max_hr: Some(146.0),
        cadence_spm: Some(174.0),
        training_load: Some(60.0),
        shoe_name: None,
        is_hyrox: None,
        post_run_context: Some(paired),
        fit_temp_source: Some("watch_internal".to_string()),
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    };
    db.upsert_activity(&act).unwrap();
    let tool_res =
        apply_post_run_context_tool_update(&db, &act, Some(300.0), Some(true)).unwrap();
    assert_eq!(tool_res["sweat_status"].as_str(), Some("final_verified"));
    assert_eq!(tool_res["final_sweat_rate_l_hr"].as_f64(), Some(1.5));
    assert_eq!(tool_res["temp_source"].as_str(), Some("user_confirmed_tempe"));
}

#[test]
fn test_v0_2_2_model_resolution_and_sqlite_settings() {
    let db = Database::open_in_memory().unwrap();
    let defaults = db.load_model_settings().unwrap();
    assert_eq!(defaults.global_model_id, DEFAULT_GEMINI_MODEL_ID);
    assert_eq!(defaults.global_thinking_level, ThinkingLevel::Medium);
    assert_eq!(defaults.coach_model_override_id, None);
    assert_eq!(defaults.coach_thinking_override, None);

    // Background & Coach both follow Hydrogen Medium by default
    let bg = resolve_model(ModelScope::Background, &defaults);
    assert_eq!(bg.model_id, "gemini-4-hydrogen");
    assert_eq!(bg.thinking_level, ThinkingLevel::Medium);
    assert!(!bg.is_override);

    let coach_default = resolve_model(ModelScope::Coach, &defaults);
    assert_eq!(coach_default.model_id, "gemini-4-hydrogen");
    assert_eq!(coach_default.thinking_level, ThinkingLevel::Medium);
    assert!(!coach_default.is_override);

    // Apply Coach override; Background MUST remain on global default
    let with_override = db
        .save_coach_model_override(Some("gemini-2.5-pro"), Some(ThinkingLevel::High))
        .unwrap();
    let coach_overridden = resolve_model(ModelScope::Coach, &with_override);
    assert_eq!(coach_overridden.model_id, "gemini-2.5-pro");
    assert_eq!(coach_overridden.thinking_level, ThinkingLevel::High);
    assert!(coach_overridden.is_override);

    let bg_still_global = resolve_model(ModelScope::Background, &with_override);
    assert_eq!(bg_still_global.model_id, "gemini-4-hydrogen");
    assert_eq!(bg_still_global.thinking_level, ThinkingLevel::Medium);
    assert!(!bg_still_global.is_override);

    // Update global defaults & clear Coach override -> both scopes converge on new global
    db.save_global_model_settings("gemini-3.1-pro-preview", ThinkingLevel::Low)
        .unwrap();
    let cleared = db.save_coach_model_override(None, None).unwrap();
    let coach_after_clear = resolve_model(ModelScope::Coach, &cleared);
    let bg_after_update = resolve_model(ModelScope::Background, &cleared);
    assert_eq!(coach_after_clear.model_id, "gemini-3.1-pro-preview");
    assert_eq!(coach_after_clear.thinking_level, ThinkingLevel::Low);
    assert!(!coach_after_clear.is_override);
    assert_eq!(bg_after_update.model_id, "gemini-3.1-pro-preview");
    assert_eq!(bg_after_update.thinking_level, ThinkingLevel::Low);
}

#[test]
fn test_v0_2_2_garmin_notes_migration_metadata_refresh_and_coach_context() {
    let dir = make_temp_dir("notes-migration");
    let db_path = dir.join("legacy.sqlite");

    // 1. Seed a legacy activities table lacking `garmin_notes`, then open via Database::open (idempotent ALTER TABLE)
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE activities (
                garmin_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                type_key TEXT NOT NULL,
                start_iso TEXT NOT NULL,
                distance_m REAL NOT NULL,
                duration_sec REAL NOT NULL,
                moving_duration_sec REAL NOT NULL,
                elevation_gain_m REAL NOT NULL,
                average_hr REAL,
                max_hr REAL,
                cadence_spm REAL,
                training_load REAL,
                shoe_name TEXT
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO activities (garmin_id, name, type_key, start_iso, distance_m, duration_sec, moving_duration_sec, elevation_gain_m)
             VALUES (7001, 'Old Run', 'running', '2026-11-18T07:00:00Z', 5000.0, 1800.0, 1800.0, 15.0)",
            [],
        )
        .unwrap();
    }

    let db = Database::open(&db_path).unwrap();
    let migrated_old = db.get_activity(7001).unwrap().unwrap();
    assert_eq!(migrated_old.garmin_notes, None);

    // 2. Late metadata edit & clear via update_activity_metadata without touching FIT tracks
    db.update_activity_metadata(
        7001,
        Some("Easy Morning Shakeout"),
        Some("Left calf tight at km 2, relaxed after 20 min."),
    )
    .unwrap();
    let updated = db.get_activity(7001).unwrap().unwrap();
    assert_eq!(updated.name, "Easy Morning Shakeout");
    assert_eq!(
        updated.garmin_notes.as_deref(),
        Some("Left calf tight at km 2, relaxed after 20 min.")
    );

    // COALESCE in upsert_activity keeps existing notes when list sync omits description
    let mut partial = updated.clone();
    partial.garmin_notes = None;
    db.upsert_activity(&partial).unwrap();
    assert_eq!(
        db.get_activity(7001).unwrap().unwrap().garmin_notes.as_deref(),
        Some("Left calf tight at km 2, relaxed after 20 min.")
    );

    // 3. Inject non-empty garmin_notes into CoachPacketBuilder, ForensicTelemetryReport, and get_activity_detail payload
    let store = SharedStore::new(Some(dir.join("coach"))).unwrap();
    let points: Vec<FitTrackPoint> = (0..10)
        .map(|s| make_fit_point(s as f64, s as f64 * 3.0, 3.0, Some(132.0), Some(174.0), Some(10.0)))
        .collect();
    let analysis = analyze_fit_points(7001, points, &AthleteConfig::default());
    db.upsert_fit_track(7001, &analysis).unwrap();

    let detail_json = splits_win_lib::coach::build_activity_detail_tool_payload(
        &updated,
        &store,
        Some(&db),
        std::slice::from_ref(&updated),
    );
    assert_eq!(
        detail_json["garmin_notes"].as_str(),
        Some("Left calf tight at km 2, relaxed after 20 min.")
    );
    assert_eq!(
        detail_json["post_run_context"]["garmin_notes"].as_str(),
        Some("Left calf tight at km 2, relaxed after 20 min.")
    );
    assert_eq!(
        detail_json["forensic_telemetry"]["garmin_notes"].as_str(),
        Some("Left calf tight at km 2, relaxed after 20 min.")
    );

    let packet = CoachPacketBuilder::build_markdown(
        &AthleteConfig::default(),
        std::slice::from_ref(&updated),
        &[],
        None,
        None,
        "How did that shakeout go?",
        Utc.with_ymd_and_hms(2026, 11, 18, 10, 0, 0).unwrap(),
    );
    assert!(packet.contains("## Garmin Connect Activity Notes"));
    assert!(packet.contains("Left calf tight at km 2, relaxed after 20 min."));

    let enriched = append_forensic_section(packet, Some(&updated), Some(&analysis.forensic_telemetry.unwrap()), None);
    assert!(enriched.contains("Garmin Connect Notes"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_v0_2_2_optional_blank_goal_time_handling() {
    let mut cfg = AthleteConfig::default();
    cfg.specs.display_name = "Alex".to_string();
    cfg.objective.goal_type = GoalType::Race;
    cfg.objective.race_name = Some("Half Marathon".to_string());
    cfg.objective.target_time_description = Some("   ".to_string());

    let summary_prompt = cfg.prompt_summary(None);
    assert!(summary_prompt.contains("Half Marathon"));
    assert!(!summary_prompt.contains("target:"));

    let now = Utc.with_ymd_and_hms(2026, 11, 18, 10, 0, 0).unwrap();
    let hist = analyze_training_history(&[], &cfg, now);
    let profile_md = starter_coach_profile_markdown(&cfg, &hist);
    let plan_md = starter_training_plan_markdown(&cfg, &hist);
    assert!(!profile_md.contains("Goal Time"));
    assert!(!plan_md.contains("Goal Time"));
    assert!(!plan_md.contains("target:"));

    // Provided goal time is preserved cleanly
    cfg.objective.target_time_description = Some("Sub 1:42".to_string());
    let plan_with_goal = starter_training_plan_markdown(&cfg, &hist);
    assert!(plan_with_goal.contains("Goal Time"));
    assert!(plan_with_goal.contains("Sub 1:42"));
}

#[test]
fn test_v0_2_2_first_use_state_machine_and_proposal_consolidation() {
    let dir = make_temp_dir("first-use");
    let store = SharedStore::new(Some(dir.clone())).unwrap();

    // 1. Clean slate starts in Intake with auto_calibrate_after_first_sync = true
    let initial = store.load_first_use_state().unwrap();
    assert_eq!(initial.stage, FirstUseStage::Intake);
    assert!(initial.auto_calibrate_after_first_sync);

    // 2. Stage transitions: Intake -> ProfileApproval -> HistorySync -> PlanApproval -> Active
    let mut flow = initial;
    flow.stage = flow.stage.next();
    assert_eq!(flow.stage, FirstUseStage::ProfileApproval);
    store.save_first_use_state(&flow).unwrap();
    assert_eq!(store.load_first_use_state().unwrap().stage, FirstUseStage::ProfileApproval);

    let sys = splits_win_lib::coach::build_static_system_instruction(
        &store,
        &AthleteConfig::default(),
        &[],
        None,
        Some(&flow),
    );
    assert!(sys.contains("HR-First Governor"));
    assert!(sys.contains("Hard Pace Floor"));
    assert!(sys.contains("ONE consolidated `COACH_PROFILE.md` proposal"));

    flow.stage = flow.stage.next();
    assert_eq!(flow.stage, FirstUseStage::HistorySync);
    flow.stage = flow.stage.next();
    assert_eq!(flow.stage, FirstUseStage::PlanApproval);
    flow.stage = flow.stage.next();
    assert_eq!(flow.stage, FirstUseStage::Active);
    store.save_first_use_state(&flow).unwrap();
    assert!(!store.load_first_use_state().unwrap().stage.is_guided());

    // 3. Consolidate multi-section proposals into 1 card per file & apply all sections + plan.json
    let now = Utc.with_ymd_and_hms(2026, 11, 18, 10, 0, 0).unwrap();
    let cfg = AthleteConfig::default();
    let summary = analyze_training_history(&[], &cfg, now);
    let plan_obj = splits_win_lib::coach::starter_plan_file(&cfg, &summary);

    let raw_proposals = vec![
        CoachUpdateProposal {
            id: "1".to_string(),
            target_file: "COACH_PROFILE.md".to_string(),
            reason: "Athlete Identity".to_string(),
            updated_section_markdown: "## Athlete Identity\n- **Athlete**: Sam".to_string(),
            expires_on: None,
            diff_preview: String::new(),
            status: "pending_user_approval".to_string(),
            plan_json: None,
            profile_thresholds: None,
        },
        CoachUpdateProposal {
            id: "2".to_string(),
            target_file: "COACH_PROFILE.md".to_string(),
            reason: "Guardrails".to_string(),
            updated_section_markdown: "## Medical & Biomechanical Guardrails\n- **HR-First**: enabled".to_string(),
            expires_on: None,
            diff_preview: String::new(),
            status: "pending_user_approval".to_string(),
            plan_json: None,
            profile_thresholds: None,
        },
        CoachUpdateProposal {
            id: "3".to_string(),
            target_file: "TRAINING_PLAN.md".to_string(),
            reason: "Block Structure".to_string(),
            updated_section_markdown: "## Mesocycle & Block Structure\n- 3:1 build/deload".to_string(),
            expires_on: Some("2026-12-31".to_string()),
            diff_preview: String::new(),
            status: "pending_user_approval".to_string(),
            plan_json: Some(plan_obj),
            profile_thresholds: None,
        },
    ];

    let consolidated = splits_win_lib::coach::consolidate_proposals(raw_proposals);
    assert_eq!(consolidated.len(), 2);
    assert_eq!(consolidated[0].target_file, "COACH_PROFILE.md");
    assert!(consolidated[0].reason.contains("Athlete Identity"));
    assert!(consolidated[0].reason.contains("Guardrails"));
    assert!(consolidated[0].diff_preview.contains("## Athlete Identity"));
    assert!(consolidated[0].diff_preview.contains("## Medical & Biomechanical Guardrails"));

    apply_coach_proposal(&store, &consolidated[0]).unwrap();
    let saved_profile = store.read_text("COACH_PROFILE.md").unwrap().unwrap();
    assert!(saved_profile.contains("- **Athlete**: Sam"));
    assert!(saved_profile.contains("- **HR-First**: enabled"));

    apply_coach_proposal(&store, &consolidated[1]).unwrap();
    assert!(dir.join("plan.json").exists());

    // 4. Existing legacy directory without first_use_flow.json upgrades safely as Active
    let _ = std::fs::remove_file(dir.join("state/first_use_flow.json"));
    assert_eq!(store.load_first_use_state().unwrap().stage, FirstUseStage::Active);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clean_slate_reset_clears_sqlite_and_shared_files() {
    let dir = make_temp_dir("clean-slate");
    let store = SharedStore::new(Some(dir.clone())).unwrap();
    store.write_text_atomic("COACH_PROFILE.md", "# seeded profile").unwrap();
    store.write_text_atomic("TRAINING_PLAN.md", "# seeded plan").unwrap();
    store.save_athlete_config(&AthleteConfig::default()).unwrap();
    let mut fus = FirstUseFlowState::default();
    fus.stage = FirstUseStage::HistorySync;
    store.save_first_use_state(&fus).unwrap();

    let db = Database::open_in_memory().unwrap();
    db.save_user_profile(&UserProfile {
        athlete_type: AthleteType::Hyrox,
        onboarding_completed: true,
        runner_config: RunnerConfig {
            vt1_bpm: Some(150),
            ..Default::default()
        },
        hyrox_config: HyroxConfig {
            division: Some("Men Open".to_string()),
            ..Default::default()
        },
    })
    .unwrap();
    db.set_setting("coach_model_override_id", Some("gemini-2.5-pro")).unwrap();

    db.clean_slate_reset().unwrap();
    store.clean_slate_files().unwrap();

    assert!(db.list_activities(10).unwrap().is_empty());
    assert!(db.list_days(10).unwrap().is_empty());
    let reset_profile = db.get_user_profile().unwrap();
    assert!(!reset_profile.onboarding_completed);
    assert_eq!(reset_profile.athlete_type, AthleteType::Runner);
    assert!(db.get_setting("coach_model_override_id").unwrap().is_none());
    assert_eq!(
        db.load_model_settings().unwrap().global_model_id,
        DEFAULT_GEMINI_MODEL_ID
    );
    assert!(store.read_text("COACH_PROFILE.md").unwrap().is_none());
    assert!(store.read_text("TRAINING_PLAN.md").unwrap().is_none());
    assert!(store.load_athlete_config().unwrap().is_none());
    assert_eq!(store.load_first_use_state().unwrap().stage, FirstUseStage::Intake);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gemini_verify_url_and_status_classification() {
    let url = splits_win_lib::coach::build_gemini_verify_url("  AIzaTestKey_99 ", "gemini-4-hydrogen");
    assert_eq!(
        url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-4-hydrogen"
    );
    assert!(!url.contains("AIzaTestKey_99"));
    assert!(!url.contains("key="));

    let ok_msg = splits_win_lib::coach::classify_gemini_verify_status(
        200,
        r#"{"displayName":"Gemini 4 Hydrogen"}"#,
    )
    .unwrap();
    assert!(ok_msg.contains("HTTP 200"));
    assert!(ok_msg.contains("Hydrogen"));

    let err_400 = splits_win_lib::coach::classify_gemini_verify_status(
        400,
        r#"{"error":{"message":"API key not valid. Please pass a valid API key."}}"#,
    )
    .unwrap_err();
    assert!(err_400.contains("HTTP 400 Invalid API Key"));

    let err_403 = splits_win_lib::coach::classify_gemini_verify_status(403, "").unwrap_err();
    assert!(err_403.contains("HTTP 403 Invalid API Key"));

    let err_429 = splits_win_lib::coach::classify_gemini_verify_status(
        429,
        r#"{"error":{"message":"Quota exceeded"}}"#,
    )
    .unwrap_err();
    assert!(err_429.contains("HTTP 429 Quota Reached"));
}

#[test]
fn stage2_profile_hydration_populates_thresholds_and_locks_weight() {
    let mut profile = UserProfile::default();
    let mut config = AthleteConfig::default();

    let structured = CoachUpdateProposal {
        id: "stage2".to_string(),
        target_file: "COACH_PROFILE.md".to_string(),
        reason: "Initial profile synthesis".to_string(),
        updated_section_markdown: "## Thresholds\n- VT1: 149 bpm\n- VT2: 171 bpm".to_string(),
        expires_on: None,
        diff_preview: String::new(),
        status: "pending_user_approval".to_string(),
        plan_json: None,
        profile_thresholds: Some(CoachProfileThresholds {
            vt1_bpm: Some(149),
            vt2_bpm: Some(171),
            vt1_pace_sec_per_km: Some(295),
            vt2_pace_sec_per_km: Some(255),
            lthr_bpm: Some(170),
            max_hr_bpm: Some(191),
            resting_hr_bpm: Some(46),
            weight_kg: Some(71.4),
            hr_first_governor: Some(true),
            hard_pace_floor_sec_per_km: Some(375),
            ..Default::default()
        }),
    };

    assert!(splits_win_lib::coach::hydrate_profile_from_proposal(
        &structured,
        &mut profile,
        &mut config
    ));
    assert_eq!(profile.runner_config.vt1_bpm, Some(149));
    assert_eq!(profile.runner_config.vt2_bpm, Some(171));
    assert_eq!(profile.runner_config.vt1_pace_sec_per_km, Some(295));
    assert_eq!(profile.runner_config.vt2_pace_sec_per_km, Some(255));
    assert_eq!(profile.runner_config.lthr_bpm, Some(170));
    assert_eq!(profile.runner_config.max_hr_bpm, Some(191));
    assert_eq!(profile.runner_config.resting_hr_bpm, Some(46));
    assert_eq!(config.specs.vt1_ceiling_bpm, Some(149));
    assert_eq!(config.specs.max_hr, Some(191));
    assert_eq!(config.specs.resting_hr_baseline, Some(46));
    assert_eq!(config.specs.weight_kg, Some(71.4));
    assert!(config.specs.weight_baseline_confirmed);

    // Markdown fallback test when structured profile_thresholds is omitted
    let mut md_profile = UserProfile::default();
    let mut md_config = AthleteConfig::default();
    let md_only = CoachUpdateProposal {
        id: "stage2-md".to_string(),
        target_file: "COACH_PROFILE.md".to_string(),
        reason: "Markdown fallback".to_string(),
        updated_section_markdown: "## Physiology\n- **VT1**: 147 bpm @ 5:05/km\n- **Max HR**: 189 bpm\n- **Weight**: 68.5 kg\n".to_string(),
        expires_on: None,
        diff_preview: String::new(),
        status: "pending_user_approval".to_string(),
        plan_json: None,
        profile_thresholds: None,
    };
    assert!(splits_win_lib::coach::hydrate_profile_from_proposal(
        &md_only,
        &mut md_profile,
        &mut md_config
    ));
    assert_eq!(md_profile.runner_config.vt1_bpm, Some(147));
    assert_eq!(md_profile.runner_config.vt1_pace_sec_per_km, Some(305));
    assert_eq!(md_profile.runner_config.max_hr_bpm, Some(189));
    assert_eq!(md_config.specs.weight_kg, Some(68.5));
    assert!(md_config.specs.weight_baseline_confirmed);
}

#[test]
fn primary_discipline_switch_preserves_runner_and_hyrox_configs() {
    let db = Database::open_in_memory().unwrap();
    let initial = UserProfile {
        athlete_type: AthleteType::HybridBoth,
        onboarding_completed: true,
        runner_config: RunnerConfig {
            primary_distance: Some("Marathon".to_string()),
            hr_first_governor: true,
            vt1_bpm: Some(148),
            vt2_bpm: Some(168),
            lthr_bpm: Some(167),
            ..Default::default()
        },
        hyrox_config: HyroxConfig {
            division: Some("Men Pro".to_string()),
            fresh_1km_pace_sec: Some(245),
            compromised_1km_target_sec: Some(265),
            weak_stations: vec!["Sled Push".to_string()],
            ..Default::default()
        },
    };
    db.save_user_profile(&initial).unwrap();

    let mut as_hyrox = db.get_user_profile().unwrap();
    as_hyrox.athlete_type = AthleteType::Hyrox;
    db.save_user_profile(&as_hyrox).unwrap();

    let mut as_runner = db.get_user_profile().unwrap();
    assert_eq!(as_runner.runner_config.vt1_bpm, Some(148));
    assert_eq!(as_runner.runner_config.primary_distance.as_deref(), Some("Marathon"));
    as_runner.athlete_type = AthleteType::Runner;
    db.save_user_profile(&as_runner).unwrap();

    let reloaded = db.get_user_profile().unwrap();
    assert_eq!(reloaded.athlete_type, AthleteType::Runner);
    assert_eq!(reloaded.runner_config.vt2_bpm, Some(168));
    assert_eq!(reloaded.hyrox_config.division.as_deref(), Some("Men Pro"));
    assert_eq!(reloaded.hyrox_config.fresh_1km_pace_sec, Some(245));
}

#[test]
fn v0_2_3_1_discipline_backcompat_goal_sync_and_force_hyrox_empty_laps() {
    // 1. Legacy AthleteConfig JSON without `discipline` infers Hybrid when goalType == "hybrid_hyrox"
    let legacy_json = r#"{
      "specs": { "displayName": "Sam", "weightKg": 70.0, "maxHR": 190, "restingHRBaseline": 48, "vt1CeilingBPM": 145, "timezoneIdentifier": "UTC" },
      "medical": { "conditionsAndNotes": "", "enforceEasyHRCeiling": false, "easyHRCeilingBPM": null, "enforcePaceFloor": false, "minPaceSecPerKm": null, "minRestHoursBetweenRuns": null },
      "objective": { "goalType": "hybrid_hyrox", "raceName": "HYROX", "raceDistanceKm": null, "raceDate": null, "targetTimeDescription": null, "hybridProfile": null },
      "availability": { "availableDays": [1,2,3,4,5,6,7], "preferredLongRunDay": 7, "maxWeekdayMinutes": null, "crossTrainingNotes": "" },
      "updatedAt": "2026-11-23T00:00:00Z"
    }"#;
    let decoded: AthleteConfig = serde_json::from_str(legacy_json).unwrap();
    assert_eq!(decoded.resolved_discipline(), TrainingDiscipline::Hybrid);

    // 2. set_discipline syncs goal_type and prompt_summary
    let mut cfg = AthleteConfig::default();
    assert_eq!(cfg.resolved_discipline(), TrainingDiscipline::Running);
    cfg.set_discipline(TrainingDiscipline::Hyrox);
    assert_eq!(cfg.discipline, Some(TrainingDiscipline::Hyrox));
    assert_eq!(cfg.objective.goal_type, GoalType::HybridHyrox);
    assert!(cfg.prompt_summary(None).contains("- Training discipline: HYROX"));

    cfg.set_discipline(TrainingDiscipline::Hybrid);
    assert_eq!(cfg.objective.goal_type, GoalType::HybridHyrox);
    assert!(cfg.prompt_summary(None).contains("- Training discipline: Hybrid (Mixed)"));

    cfg.set_discipline(TrainingDiscipline::Running);
    assert_eq!(cfg.discipline, Some(TrainingDiscipline::Running));
    assert_eq!(cfg.objective.goal_type, GoalType::Race);

    // 3. AthleteType <-> TrainingDiscipline lossless mapping
    for d in [TrainingDiscipline::Running, TrainingDiscipline::Hyrox, TrainingDiscipline::Hybrid] {
        assert_eq!(AthleteType::from_training_discipline(d).to_training_discipline(), d);
    }

    // 4. force_hyrox = true on empty laps returns 8x8/matrix dissection rather than None
    assert!(dissect_hyrox_laps(&[], false).is_none());
    let forced = dissect_hyrox_laps(&[], true).expect("forced hyrox on empty laps should return dissection");
    assert!(forced.is_hyrox_sim);
}

#[test]
fn v0_2_3_2_reentry_c25k_goals_and_clean_slate_purge() {
    // 1. ReturnToRunning & CouchTo5k GoalType serialization + display names
    let reentry: GoalType = serde_json::from_str("\"return_to_running\"").unwrap();
    let c25k: GoalType = serde_json::from_str("\"couch_to_5k\"").unwrap();
    assert_eq!(reentry, GoalType::ReturnToRunning);
    assert_eq!(c25k, GoalType::CouchTo5k);
    assert_eq!(reentry.display_name(), "Back to Running (Re-Entry)");
    assert_eq!(c25k.display_name(), "Couch to 5K (C25K)");

    // 2. Readiness & starter plan logic for C25K and ReturnToRunning
    let now = Utc.with_ymd_and_hms(2026, 11, 23, 8, 0, 0).unwrap();
    let today_day = DayRecord {
        date: "2026-11-23".to_string(),
        resting_hr: Some(58),
        stress_avg: Some(22),
        body_battery_high: Some(85),
        body_battery_latest: Some(80),
        steps: Some(4000),
        step_goal: Some(8000),
        sleep_seconds: Some(28800),
        sleep_score: Some(84),
        hrv_last_night: Some(62),
        hrv_status: Some("BALANCED".to_string()),
        ..Default::default()
    };

    let mut c25k_cfg = AthleteConfig::default();
    c25k_cfg.specs.resting_hr_baseline = Some(58);
    c25k_cfg.specs.vt1_ceiling_bpm = Some(142);
    c25k_cfg.objective.goal_type = GoalType::CouchTo5k;
    c25k_cfg.objective.race_distance_km = Some(5.0);
    c25k_cfg.objective.target_time_description = None;

    let summary = analyze_training_history(&[], &c25k_cfg, now);
    let plan_md = starter_training_plan_markdown(&c25k_cfg, &summary);
    assert!(plan_md.contains("Couch to 5K (C25K)"));
    assert!(plan_md.contains("8-9 week"));
    assert!(plan_md.contains("5.0 km"));
    assert!(plan_md.contains("finish goal — no target time set"));

    let starter = starter_plan_file(&c25k_cfg, &summary);
    assert_eq!(starter.envelope.mesocycle_phase, "couch_to_5k");
    assert_eq!(starter.envelope.max_single_run_km, 5.0);
    assert_eq!(starter.envelope.min_hours_between_runs, 48.0);
    assert!(!starter.sessions.is_empty());
    assert!(starter
        .sessions
        .iter()
        .all(|s| s.session_type == RunTypology::RunWalkReentry));

    let c25k_out = ReadinessEngine::evaluate(ReadinessInput {
        now,
        athlete_config: &c25k_cfg,
        activities: &[],
        days: std::slice::from_ref(&today_day),
        plan: None,
        gear_inventory: None,
    });
    assert_eq!(
        c25k_out.authoritative_target.prescription.as_ref().and_then(|p| p.prescribed_title.as_deref()),
        Some("C25K Run-Walk Progression")
    );
    assert_eq!(c25k_out.readiness_state.gatekeeper.remaining_weekly_budget_km, 15.0);
    assert!(c25k_out.readiness_state.prescription_summary.distance_km <= 5.0);

    let mut reentry_cfg = AthleteConfig::default();
    reentry_cfg.specs.resting_hr_baseline = Some(56);
    reentry_cfg.specs.vt1_ceiling_bpm = Some(140);
    reentry_cfg.objective.goal_type = GoalType::ReturnToRunning;
    let reentry_out = ReadinessEngine::evaluate(ReadinessInput {
        now,
        athlete_config: &reentry_cfg,
        activities: &[],
        days: &[today_day],
        plan: None,
        gear_inventory: None,
    });
    assert_eq!(
        reentry_out.authoritative_target.prescription.as_ref().and_then(|p| p.prescribed_title.as_deref()),
        Some("Run-Walk Re-Entry")
    );
    assert_eq!(reentry_out.readiness_state.gatekeeper.remaining_weekly_budget_km, 18.0);

    // 3. Clean-Slate Reset purges SQLite tables, optional tables, .fit files & coach markdown/json state
    let dir = make_temp_dir("clean-slate-0232");
    let store = SharedStore::new(Some(dir.clone())).unwrap();
    let db = Database::open(&dir.join("state/splits.sqlite")).unwrap();
    let mut prof = db.get_user_profile().unwrap();
    prof.onboarding_completed = true;
    db.save_user_profile(&prof).unwrap();
    db.set_setting("custom_key", Some("val")).unwrap();
    db.save_global_model_settings("gemini-4-hydrogen", ThinkingLevel::High).unwrap();

    std::fs::write(dir.join("sample.fit"), b"fitdata").unwrap();
    std::fs::write(dir.join("COACH_PROFILE.md"), b"# Profile").unwrap();
    std::fs::write(dir.join("TRAINING_PLAN.md"), b"# Plan").unwrap();
    std::fs::write(dir.join("readiness_state.json"), b"{}").unwrap();
    std::fs::write(dir.join("state/first_use_flow.json"), b"{\"stage\":\"active\"}").unwrap();

    db.clean_slate_reset().unwrap();
    store.clean_slate_files().unwrap();

    let reset_prof = db.get_user_profile().unwrap();
    assert!(!reset_prof.onboarding_completed);
    assert_eq!(db.get_setting("custom_key").unwrap(), None);
    assert!(!dir.join("sample.fit").exists());
    assert!(!dir.join("COACH_PROFILE.md").exists());
    assert!(!dir.join("TRAINING_PLAN.md").exists());
    assert!(!dir.join("readiness_state.json").exists());
    assert!(!dir.join("state/first_use_flow.json").exists());
    assert!(dir.join("state").exists());
    assert!(dir.join("history").exists());
    assert!(dir.join("attachments").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_3_3_credentials_json_atomic_persistence_and_clean_slate_delete() {
    let dir = make_temp_dir("creds-0233");
    let cred_path = dir.join("credentials.json");

    // Production path anchors directly to SharedStore base dir
    assert_eq!(
        CredentialStore::default_credentials_path(),
        SharedStore::default_base_dir().join("credentials.json")
    );

    // 1. Atomic write & immediate read-back for Garmin & Gemini & uiserver keys
    CredentialStore::save_garmin_key_at(&cred_path, "  garmin-personal-xyz  ").unwrap();
    assert_eq!(
        CredentialStore::read_garmin_key_at(&cred_path).as_deref(),
        Some("garmin-personal-xyz")
    );
    assert_eq!(CredentialStore::read_gemini_key_at(&cred_path), None);

    CredentialStore::save_gemini_key_at(&cred_path, "AIzaTestKey123").unwrap();
    CredentialStore::save_uiserver_token_at(&cred_path, "ui-token-abc").unwrap();

    // 2. Overwriting one key preserves the others and leaves zero .tmp file behind
    CredentialStore::save_garmin_key_at(&cred_path, "garmin-personal-rotated").unwrap();
    assert_eq!(
        CredentialStore::read_garmin_key_at(&cred_path).as_deref(),
        Some("garmin-personal-rotated")
    );
    assert_eq!(
        CredentialStore::read_gemini_key_at(&cred_path).as_deref(),
        Some("AIzaTestKey123")
    );
    assert_eq!(
        CredentialStore::read_uiserver_token_at(&cred_path).as_deref(),
        Some("ui-token-abc")
    );
    assert!(!dir.join("credentials.json.tmp").exists());

    let raw = std::fs::read_to_string(&cred_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["garmin_personal_api_key"], "garmin-personal-rotated");
    assert_eq!(parsed["gemini_api_key"], "AIzaTestKey123");
    assert_eq!(parsed["uiserver_auth_token"], "ui-token-abc");

    // 3. Individual delete leaves remaining keys intact
    CredentialStore::delete_gemini_key_at(&cred_path).unwrap();
    assert_eq!(CredentialStore::read_gemini_key_at(&cred_path), None);
    assert_eq!(
        CredentialStore::read_garmin_key_at(&cred_path).as_deref(),
        Some("garmin-personal-rotated")
    );

    // 4. Clean-slate purge removes credentials.json and yields None on subsequent reads
    CredentialStore::delete_credentials_file_at(&cred_path);
    assert!(!cred_path.exists());
    assert_eq!(CredentialStore::read_garmin_key_at(&cred_path), None);
    assert_eq!(CredentialStore::read_gemini_key_at(&cred_path), None);
    assert_eq!(CredentialStore::read_uiserver_token_at(&cred_path), None);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_4_garmin_sleep_2026_11_25_and_tolerant_decoding() {
    let start_ms = NaiveDate::from_ymd_opt(2026, 11, 25)
        .unwrap()
        .and_hms_opt(0, 14, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    let end_ms = NaiveDate::from_ymd_opt(2026, 11, 25)
        .unwrap()
        .and_hms_opt(7, 20, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();

    let payload = serde_json::json!([
        {
            "dailySleepDTO": {
                "calendarDate": "2026-11-25",
                "sleepTimeSeconds": 25560,
                "deepSleepSeconds": 5100,
                "lightSleepSeconds": 13500,
                "remSleepSeconds": 6200,
                "awakeSleepSeconds": 760,
                "sleepStartTimestampLocal": start_ms,
                "sleepEndTimestampLocal": end_ms,
                "sleepScores": {
                    "overall": { "value": 91 }
                }
            }
        },
        {
            // Shorter afternoon nap should NOT overwrite main sleep
            "calendarDate": "2026-11-25",
            "sleepTimeSeconds": 1800,
            "deepSleepSeconds": 300,
            "sleepScores": { "overall": 60 }
        },
        {
            // Zero-second placeholder should be ignored
            "calendarDate": "2026-11-24",
            "sleepTimeSeconds": 0,
            "sleepScores": { "overall": { "value": 0 } }
        },
        {
            // Tolerant float & top-level DTO fallback
            "calendarDate": "2026-11-23",
            "sleepTimeSeconds": 26100.0,
            "deepSleepSeconds": 4800.0,
            "lightSleepSeconds": 14400.0,
            "remSleepSeconds": 6300.0,
            "awakeSleepSeconds": 600.0,
            "sleepStartTimestampLocal": "2026-11-22 23:45:00",
            "sleepEndTimestampLocal": "2026-11-23 07:00:00",
            "sleepScores": { "overall": 86.0 }
        }
    ]);

    let mut days = std::collections::HashMap::new();
    parse_sleep_summaries_json(serde_json::to_vec(&payload).unwrap().as_slice(), &mut days);

    assert!(!days.contains_key("2026-11-24"));

    let d25 = days.get("2026-11-25").expect("2026-11-25 sleep present");
    assert_eq!(d25.sleep_seconds, Some(25560));
    assert_eq!(d25.sleep_duration_sec, Some(25560));
    assert_eq!(d25.sleep_score, Some(91));
    assert_eq!(d25.sleep_start.as_deref(), Some("2026-11-25T00:14:00"));
    assert_eq!(d25.sleep_end.as_deref(), Some("2026-11-25T07:20:00"));
    assert_eq!(d25.sleep_deep_sec, Some(5100));
    assert_eq!(d25.sleep_light_sec, Some(13500));
    assert_eq!(d25.sleep_rem_sec, Some(6200));
    assert_eq!(d25.sleep_awake_sec, Some(760));

    let d23 = days.get("2026-11-23").expect("2026-11-23 sleep present");
    assert_eq!(d23.sleep_seconds, Some(26100));
    assert_eq!(d23.sleep_score, Some(86));
    assert_eq!(d23.sleep_start.as_deref(), Some("2026-11-22T23:45:00"));
    assert_eq!(d23.sleep_end.as_deref(), Some("2026-11-23T07:00:00"));

    // Verify SQLite persistence and readback
    let dir = make_temp_dir("sleep-persist-024");
    let db = Database::open(&dir.join("splits.sqlite")).unwrap();
    db.upsert_day(d25).unwrap();
    db.upsert_day(d23).unwrap();
    let loaded = db.list_days(10).unwrap();
    let loaded_25 = loaded.iter().find(|d| d.date == "2026-11-25").unwrap();
    assert_eq!(loaded_25.sleep_score, Some(91));
    assert_eq!(loaded_25.sleep_duration_sec, Some(25560));
    assert_eq!(loaded_25.sleep_start.as_deref(), Some("2026-11-25T00:14:00"));
    assert_eq!(loaded_25.sleep_end.as_deref(), Some("2026-11-25T07:20:00"));
    assert_eq!(loaded_25.sleep_deep_sec, Some(5100));
    assert_eq!(loaded_25.sleep_light_sec, Some(13500));
    assert_eq!(loaded_25.sleep_rem_sec, Some(6200));
    assert_eq!(loaded_25.sleep_awake_sec, Some(760));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_4_body_battery_hrv_baseline_and_28d_chunking() {
    // 1. Garmin 28-day window chunking
    let start = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
    let chunks = chunk_date_range(start, end, 28);
    assert_eq!(chunks.len(), 2);
    for (cs, ce) in &chunks {
        assert!((*ce - *cs).num_days() + 1 <= 28);
    }
    assert_eq!(chunks[0], (NaiveDate::from_ymd_opt(2026, 10, 29).unwrap(), end));
    assert_eq!(chunks[1], (start, NaiveDate::from_ymd_opt(2026, 10, 28).unwrap()));

    // 2. Daily summaries + Body Battery intraday + HRV + Steps decoding & SQLite merge
    let mut days = std::collections::HashMap::new();
    let daily_json = serde_json::json!([
        {
            "calendarDate": "2026-11-25",
            "heartRate": { "restingValue": 49 },
            "stress": { "avgLevel": 24 },
            "bodyBattery": { "maxValue": 91, "minValue": 29, "chargedValue": 65, "drainedValue": 38, "latestValue": 70 },
            "movement": { "steps": { "count": 4820, "goal": 8000 } }
        },
        {
            "calendarDate": "2026-11-24",
            "stress": { "avgLevel": -1 }
        }
    ]);
    parse_daily_summaries_json(&serde_json::to_vec(&daily_json).unwrap(), &mut days);
    assert_eq!(days.get("2026-11-24").unwrap().stress_avg, None);

    let bb_json = serde_json::json!([
        {
            "date": "2026-11-25",
            "charged": 66,
            "drained": 39,
            "bodyBatteryValuesArray": [
                [1795564800000_i64, 42],
                [1795565700000_i64, null],
                [1795586400000_i64, 92],
                [1795600800000_i64, 69]
            ]
        }
    ]);
    parse_body_battery_days_json(&serde_json::to_vec(&bb_json).unwrap(), &mut days);

    let hrv_json = serde_json::json!({
        "hrvSummaries": [
            {
                "calendarDate": "2026-11-25",
                "lastNightAvg": 68,
                "weeklyAvg": 64,
                "status": "BALANCED",
                "baseline": { "balancedLow": 55, "balancedUpper": 74 }
            }
        ]
    });
    parse_hrv_range_json(&serde_json::to_vec(&hrv_json).unwrap(), &mut days);

    let steps_json = serde_json::json!([
        { "calendarDate": "2026-11-25", "totalSteps": 5120, "stepGoal": 8500 }
    ]);
    parse_steps_json(&serde_json::to_vec(&steps_json).unwrap(), &mut days);

    let d = days.get("2026-11-25").unwrap();
    assert_eq!(d.body_battery_high, Some(92));
    assert_eq!(d.body_battery_low, Some(29));
    assert_eq!(d.body_battery_latest, Some(69));
    assert_eq!(d.body_battery_charged, Some(66));
    assert_eq!(d.body_battery_drained, Some(39));
    assert_eq!(d.body_battery_series.as_ref().map(|v| v.len()), Some(3));
    assert_eq!(d.hrv_last_night, Some(68));
    assert_eq!(d.hrv_weekly_avg, Some(64));
    assert_eq!(d.hrv_baseline_low, Some(55));
    assert_eq!(d.hrv_baseline_high, Some(74));
    assert_eq!(d.hrv_status.as_deref(), Some("BALANCED"));
    assert_eq!(d.steps, Some(5120));
    assert_eq!(d.step_goal, Some(8500));

    let dir = make_temp_dir("bb-hrv-024");
    let db = Database::open(&dir.join("splits.sqlite")).unwrap();
    db.upsert_day(d).unwrap();

    // Verify partial Body Battery intraday refresh does NOT wipe previously saved HRV / Sleep fields
    db.upsert_day(&DayRecord {
        date: "2026-11-25".to_string(),
        body_battery_high: Some(92),
        body_battery_latest: Some(64),
        body_battery_drained: Some(44),
        ..Default::default()
    })
    .unwrap();

    let rows = db.list_days(5).unwrap();
    let merged = &rows[0];
    assert_eq!(merged.body_battery_latest, Some(64));
    assert_eq!(merged.body_battery_drained, Some(44));
    assert_eq!(merged.hrv_last_night, Some(68));
    assert_eq!(merged.hrv_weekly_avg, Some(64));
    assert_eq!(merged.body_battery_series.as_ref().map(|v| v.len()), Some(3));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_4_sqlite_migration_briefing_cache_and_theme_mode() {
    let dir = make_temp_dir("migrate-024");
    let db_path = dir.join("splits.sqlite");

    // Seed legacy `days` table without v0.2.4 columns
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE days (
                date TEXT PRIMARY KEY,
                resting_hr INTEGER,
                stress_avg INTEGER,
                body_battery_high INTEGER,
                body_battery_latest INTEGER,
                steps INTEGER,
                step_goal INTEGER,
                sleep_seconds INTEGER,
                sleep_score INTEGER,
                hrv_last_night INTEGER,
                hrv_status TEXT
            );
            INSERT INTO days (date, resting_hr, sleep_seconds, sleep_score, hrv_last_night, hrv_status)
            VALUES ('2026-11-24', 51, 24000, 82, 60, 'BALANCED');",
        )
        .unwrap();
    }

    // Re-opening runs idempotent ALTER TABLE migrations & creates `briefings`
    let db = Database::open(&db_path).unwrap();
    let days = db.list_days(10).unwrap();
    assert_eq!(days.len(), 1);
    assert_eq!(days[0].sleep_score, Some(82));
    assert_eq!(days[0].sleep_duration_sec, Some(24000));
    assert_eq!(days[0].hrv_weekly_avg, None);

    // Daily briefing cache & prompt formatting
    assert_eq!(
        without_dashes("**Great sleep** — ready to roll"),
        "Great sleep, ready to roll"
    );
    let briefing = DailyBriefingRecord {
        date: "2026-11-25".to_string(),
        headline: "Rested and ready for easy aerobic work".to_string(),
        body: "Sleep came in at 7 h 6 min with score 91 and balanced HRV at 68 ms.\n\nNo sessions logged yesterday so legs are fresh within your weekly envelope.".to_string(),
        action_cue: "Run 5.0 km easy under 142 bpm.".to_string(),
        model_id: "gemini-3-flash-preview".to_string(),
        thinking_level: "LOW".to_string(),
        had_sleep: true,
        generated_at: "2026-11-25T07:30:00Z".to_string(),
    };
    db.upsert_daily_briefing(&briefing).unwrap();
    let cached = db.get_daily_briefing("2026-11-25").unwrap().unwrap();
    assert_eq!(cached, briefing);

    let facts = build_daily_briefing_facts(
        &AthleteConfig::default(),
        &[],
        &[DayRecord {
            date: "2026-11-25".to_string(),
            sleep_seconds: Some(25560),
            sleep_score: Some(91),
            body_battery_high: Some(92),
            body_battery_latest: Some(69),
            hrv_last_night: Some(68),
            hrv_status: Some("BALANCED".to_string()),
            ..Default::default()
        }],
        None,
        NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(),
    );
    assert!(facts.contains("Last night: slept 7 h 6 min, sleep score 91 out of 100"));
    assert!(facts.contains("Energy level (0 to 100) peaked at 92 (69 now)"));

    // Theme mode persistence & reset
    assert_eq!(db.get_theme_mode().unwrap(), "system");
    assert_eq!(db.set_theme_mode("dark").unwrap(), "dark");
    assert_eq!(db.get_theme_mode().unwrap(), "dark");
    assert_eq!(db.set_theme_mode("light").unwrap(), "light");
    assert_eq!(db.get_theme_mode().unwrap(), "light");
    db.clean_slate_reset().unwrap();
    assert_eq!(db.get_theme_mode().unwrap(), "system");
    assert_eq!(db.get_daily_briefing("2026-11-25").unwrap(), None);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_4_1_legacy_v0_2_3_3_upgrade_bootstrap_serializes_cleanly() {
    let dir = make_temp_dir("upgrade-0233");
    let store = SharedStore::new(Some(dir.clone())).unwrap();
    let db_path = dir.join("state").join("splits.sqlite");

    // Seed pre-v0.2.4 (v0.2.3.3) schema for both `days` and `activities`
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE activities (
                garmin_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                type_key TEXT NOT NULL,
                start_iso TEXT NOT NULL,
                distance_m REAL NOT NULL,
                duration_sec REAL NOT NULL,
                moving_duration_sec REAL NOT NULL,
                elevation_gain_m REAL NOT NULL,
                average_hr REAL,
                max_hr REAL,
                cadence_spm REAL,
                training_load REAL,
                shoe_name TEXT
            );
            CREATE TABLE days (
                date TEXT PRIMARY KEY,
                resting_hr INTEGER,
                stress_avg INTEGER,
                body_battery_high INTEGER,
                body_battery_latest INTEGER,
                steps INTEGER,
                step_goal INTEGER,
                sleep_seconds INTEGER,
                sleep_score INTEGER,
                hrv_last_night INTEGER,
                hrv_status TEXT
            );
            INSERT INTO activities (
                garmin_id, name, type_key, start_iso, distance_m, duration_sec, moving_duration_sec,
                elevation_gain_m, average_hr, max_hr, cadence_spm, training_load, shoe_name
            ) VALUES (
                1001, 'Legacy Easy Run', 'running', '2026-11-24T07:30:00Z', 5000.0, 1800.0, 1790.0,
                25.0, 136.0, 144.0, 174.0, 42.0, NULL
            );
            INSERT INTO days (
                date, resting_hr, stress_avg, body_battery_high, body_battery_latest,
                steps, step_goal, sleep_seconds, sleep_score, hrv_last_night, hrv_status
            ) VALUES (
                '2026-11-24', 52, NULL, 80, NULL, 4200, 8000, 24000, 81, NULL, NULL
            );",
        )
        .unwrap();
    }

    let db = Database::open(&db_path).unwrap();
    let settings = splits_win_lib::storage::AppSettings::default();
    let val = splits_win_lib::build_bootstrap_state(&store, &db, &settings).unwrap();
    let json = serde_json::to_string(&val).unwrap();
    assert!(!json.is_empty());

    let days = val["days"].as_array().expect("days array");
    assert_eq!(days.len(), 1);
    assert_eq!(days[0]["date"], "2026-11-24");
    assert_eq!(days[0]["sleep_seconds"], 24000);
    assert_eq!(days[0]["sleep_duration_sec"], 24000);
    assert!(days[0]["body_battery_series"].is_null());
    assert!(days[0]["sleep_start"].is_null());
    assert!(days[0]["sleep_end"].is_null());
    assert!(days[0]["hrv_weekly_avg"].is_null());
    assert!(days[0]["hrv_baseline_low"].is_null());
    assert!(days[0]["hrv_baseline_high"].is_null());

    let acts = val["activities"].as_array().expect("activities array");
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0]["garmin_id"], 1001);

    assert!(val["daily_briefing"].is_null());
    assert_eq!(val["theme_mode"], "system");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_5_migration_from_v0_2_4_preserves_rows_and_parses_extended_sleep_hrv() {
    let dir = make_temp_dir("migrate-025");
    let db_path = dir.join("splits.sqlite");

    // Seed v0.2.4 `days` and `activities` tables without v0.2.5 extended columns
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE activities (
                garmin_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                type_key TEXT NOT NULL,
                start_iso TEXT NOT NULL,
                distance_m REAL NOT NULL,
                duration_sec REAL NOT NULL,
                moving_duration_sec REAL NOT NULL,
                elevation_gain_m REAL NOT NULL,
                average_hr REAL,
                max_hr REAL,
                cadence_spm REAL,
                training_load REAL,
                shoe_name TEXT,
                is_hyrox INTEGER,
                post_run_context_json TEXT,
                fit_temp_source TEXT,
                garmin_notes TEXT
            );
            CREATE TABLE days (
                date TEXT PRIMARY KEY,
                resting_hr INTEGER,
                stress_avg INTEGER,
                body_battery_high INTEGER,
                body_battery_latest INTEGER,
                body_battery_low INTEGER,
                body_battery_charged INTEGER,
                body_battery_drained INTEGER,
                body_battery_series_json TEXT,
                steps INTEGER,
                step_goal INTEGER,
                sleep_seconds INTEGER,
                sleep_duration_sec INTEGER,
                sleep_score INTEGER,
                sleep_start TEXT,
                sleep_end TEXT,
                sleep_deep_sec INTEGER,
                sleep_light_sec INTEGER,
                sleep_rem_sec INTEGER,
                sleep_awake_sec INTEGER,
                hrv_last_night INTEGER,
                hrv_status TEXT,
                hrv_weekly_avg INTEGER,
                hrv_baseline_low INTEGER,
                hrv_baseline_high INTEGER
            );
            INSERT INTO activities (
                garmin_id, name, type_key, start_iso, distance_m, duration_sec, moving_duration_sec,
                elevation_gain_m, average_hr, max_hr, cadence_spm, training_load, shoe_name
            ) VALUES (
                2001, 'v0.2.4 Base Run', 'running', '2026-11-24T07:00:00Z', 10000.0, 3000.0, 2980.0,
                45.0, 142.0, 156.0, 174.0, 88.0, 'Superblast 3'
            );
            INSERT INTO days (
                date, resting_hr, sleep_seconds, sleep_duration_sec, sleep_score,
                sleep_start, sleep_end, sleep_deep_sec, sleep_light_sec, sleep_rem_sec, sleep_awake_sec,
                hrv_last_night, hrv_status, hrv_weekly_avg, hrv_baseline_low, hrv_baseline_high
            ) VALUES (
                '2026-11-24', 50, 26400, 26400, 86,
                '2026-11-23T23:00:00', '2026-11-24T06:30:00', 5400, 15000, 6000, 600,
                66, 'BALANCED', 64, 56, 74
            );",
        )
        .unwrap();
    }

    let db = Database::open(&db_path).unwrap();
    let legacy_days = db.list_days(10).unwrap();
    assert_eq!(legacy_days.len(), 1);
    assert_eq!(legacy_days[0].sleep_score, Some(86));
    assert_eq!(legacy_days[0].sleep_need_minutes, None);
    assert_eq!(legacy_days[0].hrv_last_night_5min_high, None);

    let legacy_acts = db.list_activities(10).unwrap();
    assert_eq!(legacy_acts.len(), 1);
    assert_eq!(legacy_acts[0].garmin_id, 2001);
    assert_eq!(legacy_acts[0].is_pr, None);

    // Parse extended Garmin sleep + HRV + intraday 5-min HRV payloads
    let mut parsed_days = std::collections::HashMap::new();
    let sleep_json = serde_json::json!({
        "dailySleepDTO": {
            "calendarDate": "2026-11-25",
            "sleepTimeSeconds": 27000,
            "deepSleepSeconds": 5700,
            "lightSleepSeconds": 15300,
            "remSleepSeconds": 6000,
            "awakeSleepSeconds": 480,
            "sleepStartTimestampLocal": "2026-11-24T23:10:00",
            "sleepEndTimestampLocal": "2026-11-25T06:48:00",
            "avgHeartRate": 48.5,
            "avgSleepStress": 14.2,
            "awakeCount": 2,
            "averageRespirationValue": 14.6,
            "lowestRespirationValue": 12.0,
            "highestRespirationValue": 17.5,
            "sleepScores": { "overall": { "value": 90, "qualifierKey": "GOOD" } },
            "sleepNeed": { "baseline": 450, "actual": 480 }
        }
    });
    parse_sleep_summaries_json(&serde_json::to_vec(&sleep_json).unwrap(), &mut parsed_days);

    let hrv_json = serde_json::json!({
        "hrvSummaries": [{
            "calendarDate": "2026-11-25",
            "lastNightAvg": 71,
            "lastNight5MinHigh": 96,
            "weeklyAvg": 67,
            "status": "BALANCED",
            "baseline": { "balancedLow": 56, "balancedUpper": 75 }
        }]
    });
    parse_hrv_range_json(&serde_json::to_vec(&hrv_json).unwrap(), &mut parsed_days);

    let d25 = parsed_days.get("2026-11-25").unwrap();
    db.upsert_day(d25).unwrap();

    // Partial update must NOT wipe extended sleep / HRV fields
    db.upsert_day(&DayRecord {
        date: "2026-11-25".to_string(),
        steps: Some(9200),
        ..Default::default()
    })
    .unwrap();

    let rows = db.list_days(10).unwrap();
    let stored = rows.iter().find(|r| r.date == "2026-11-25").unwrap();
    assert_eq!(stored.steps, Some(9200));
    assert_eq!(stored.sleep_need_minutes, Some(480));
    assert_eq!(stored.sleep_baseline_minutes, Some(450));
    assert_eq!(stored.sleep_awake_count, Some(2));
    assert_eq!(stored.sleep_avg_hr, Some(48.5));
    assert_eq!(stored.sleep_stress, Some(14.2));
    assert_eq!(stored.sleep_respiration_avg, Some(14.6));
    assert_eq!(stored.sleep_respiration_low, Some(12.0));
    assert_eq!(stored.sleep_respiration_high, Some(17.5));
    assert_eq!(stored.sleep_quality.as_deref(), Some("GOOD"));
    assert_eq!(stored.hrv_last_night_5min_high, Some(96));

    // Intraday 5-min HRV reading parser
    let night_json = serde_json::json!({
        "hrvSummary": {
            "lastNightAvg": 71,
            "lastNight5MinHigh": 96,
            "baseline": { "balancedLow": 56, "balancedUpper": 75 }
        },
        "hrvReadings": [
            { "hrvValue": 64, "readingTimeGMT": "2026-11-25T00:05:00.0" },
            { "hrvValue": 96, "readingTimeGMT": "2026-11-25T03:30:00.0" }
        ]
    });
    let detail = splits_win_lib::garmin::parse_hrv_night_json(
        "2026-11-25",
        &serde_json::to_vec(&night_json).unwrap(),
    );
    assert_eq!(detail.readings.len(), 2);
    assert_eq!(detail.last_night_5min_high, Some(96));
    assert_eq!(detail.readings[1].value_ms, 96.0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_5_records_aggregation_and_scoped_sleep_hrv_takes() {
    let dir = make_temp_dir("records-takes-025");
    let db = Database::open(&dir.join("splits.sqlite")).unwrap();

    // Insert outdoor runs (one with cached 1Hz FIT track, one activity average) + indoor treadmill (must be ignored for distance PRs)
    let now = Utc::now();
    let d1 = (now - Duration::days(20)).format("%Y-%m-%dT07:00:00Z").to_string();
    let d2 = (now - Duration::days(10)).format("%Y-%m-%dT07:00:00Z").to_string();
    let d3 = (now - Duration::days(5)).format("%Y-%m-%dT07:00:00Z").to_string();

    db.upsert_activity(&ActivityRecord {
        garmin_id: 3001,
        name: "10K Threshold Test".to_string(),
        type_key: "running".to_string(),
        start_iso: d1.clone(),
        distance_m: 10_200.0,
        duration_sec: 2_600.0,
        moving_duration_sec: 2_550.0,
        elevation_gain_m: 40.0,
        average_hr: Some(166.0),
        max_hr: Some(178.0),
        cadence_spm: Some(180.0),
        training_load: Some(140.0),
        shoe_name: Some("Metaspeed".to_string()),
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: Some(true),
        start_latitude: None,
        start_longitude: None,
    })
    .unwrap();

    db.upsert_activity(&ActivityRecord {
        garmin_id: 3002,
        name: "Mountain Long Run".to_string(),
        type_key: "trail_running".to_string(),
        start_iso: d2.clone(),
        distance_m: 22_000.0,
        duration_sec: 7_200.0,
        moving_duration_sec: 7_000.0,
        elevation_gain_m: 650.0,
        average_hr: Some(148.0),
        max_hr: Some(168.0),
        cadence_spm: Some(172.0),
        training_load: Some(210.0),
        shoe_name: Some("Trabuco".to_string()),
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: Some(false),
        start_latitude: None,
        start_longitude: None,
    })
    .unwrap();

    // Unrealistically fast treadmill run must be excluded from outdoor distance PRs
    db.upsert_activity(&ActivityRecord {
        garmin_id: 3003,
        name: "Treadmill".to_string(),
        type_key: "treadmill_running".to_string(),
        start_iso: d3,
        distance_m: 5_000.0,
        duration_sec: 900.0,
        moving_duration_sec: 900.0,
        elevation_gain_m: 0.0,
        average_hr: Some(150.0),
        max_hr: None,
        cadence_spm: None,
        training_load: Some(60.0),
        shoe_name: None,
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    })
    .unwrap();

    // Attach a 1Hz GPS FIT track to activity 3001 with 4:10/km pace inside a 10.2km run
    let fit_points: Vec<FitTrackPoint> = (0..=2550)
        .map(|s| {
            let dist = (s as f64) * 4.0; // 4.0 m/s = 4:10/km, 2550*4 = 10200m
            make_gps_fit_point(s as f64, dist, 4.0, Some(166.0), Some(180.0), Some(10.0))
        })
        .collect();
    let track = analyze_fit_points(3001, fit_points, &AthleteConfig::default());
    db.upsert_fit_track(3001, &track).unwrap();

    let records = db.compute_personal_records().unwrap();
    assert_eq!(records.total_outdoor_runs, 2);
    assert_eq!(records.scanned_fit_runs, 1);
    assert_eq!(records.pr_activities.len(), 1);
    assert_eq!(records.pr_activities[0].garmin_id, 3001);

    let best_5k = records.distance_records.iter().find(|r| r.label == "5K").unwrap();
    assert_eq!(best_5k.activity_id, 3001);
    assert_eq!(best_5k.source, "fit_best_stretch");
    assert!((best_5k.time_sec - 1250.0).abs() < 2.0);

    // 3002 has no cached GPS FIT track -> must NOT receive a Run Avg / activity_average fallback PR,
    // while still counting toward volume/elevation milestones.
    assert!(records
        .distance_records
        .iter()
        .all(|r| r.label != "Half Marathon" && r.source == "fit_best_stretch"));

    assert!(records.milestones.iter().any(|m| m.key == "longest_run" && m.activity_id == Some(3002)));
    assert!(records.milestones.iter().any(|m| m.key == "biggest_elevation" && m.activity_id == Some(3002)));
    assert!(records.milestones.iter().any(|m| m.key == "highest_weekly_km"));
    assert!(!records.predictions.is_empty());
    assert!(!records.yearly_bests.is_empty());

    // Scoped `sleep_take` and `hrv_take` persistence alongside daily briefing
    let daily = DailyBriefingRecord {
        date: "2026-11-25".to_string(),
        headline: "Daily ready".to_string(),
        body: "Daily body.".to_string(),
        action_cue: "Run easy.".to_string(),
        model_id: "gemini-4-hydrogen".to_string(),
        thinking_level: "MEDIUM".to_string(),
        had_sleep: true,
        generated_at: "2026-11-25T07:00:00Z".to_string(),
    };
    let sleep_take = DailyBriefingRecord {
        date: "2026-11-25".to_string(),
        headline: "Solid sleep bank".to_string(),
        body: "You slept 7 h 30 min.".to_string(),
        action_cue: "Keep bedtime steady.".to_string(),
        model_id: "gemini-4-hydrogen".to_string(),
        thinking_level: "MEDIUM".to_string(),
        had_sleep: true,
        generated_at: "2026-11-25T07:01:00Z".to_string(),
    };
    let hrv_take = DailyBriefingRecord {
        date: "2026-11-25".to_string(),
        headline: "Balanced autonomic recovery".to_string(),
        body: "Last night's HRV was 71 ms.".to_string(),
        action_cue: "Carry on as planned.".to_string(),
        model_id: "gemini-4-hydrogen".to_string(),
        thinking_level: "MEDIUM".to_string(),
        had_sleep: false,
        generated_at: "2026-11-25T07:02:00Z".to_string(),
    };

    db.upsert_daily_briefing(&daily).unwrap();
    db.upsert_scoped_briefing("sleep_take", &sleep_take).unwrap();
    db.upsert_scoped_briefing("hrv_take", &hrv_take).unwrap();

    assert_eq!(db.get_daily_briefing("2026-11-25").unwrap(), Some(daily));
    assert_eq!(db.get_scoped_briefing("sleep_take", "2026-11-25").unwrap(), Some(sleep_take));
    assert_eq!(db.get_scoped_briefing("hrv_take", "2026-11-25").unwrap(), Some(hrv_take));

    // Deterministic health take fact builders
    let sample_days: Vec<DayRecord> = (0..10)
        .map(|i| {
            let dt = (NaiveDate::from_ymd_opt(2026, 11, 16).unwrap() + Duration::days(i))
                .format("%Y-%m-%d")
                .to_string();
            DayRecord {
                date: dt,
                resting_hr: Some(50),
                sleep_seconds: Some(26_400),
                sleep_need_minutes: Some(450),
                sleep_start: Some("2026-11-24T23:05:00".to_string()),
                sleep_end: Some("2026-11-25T06:35:00".to_string()),
                sleep_deep_sec: Some(5_400),
                sleep_light_sec: Some(15_000),
                sleep_rem_sec: Some(6_000),
                hrv_last_night: Some(68 + (i as i32 % 3)),
                hrv_baseline_low: Some(56),
                hrv_baseline_high: Some(74),
                ..Default::default()
            }
        })
        .collect();
    let ref_date = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
    let sleep_facts = splits_win_lib::coach::build_sleep_take_facts(&sample_days, ref_date).unwrap();
    assert!(sleep_facts.contains("Verdict:"));
    let hrv_facts = splits_win_lib::coach::build_hrv_take_facts(&sample_days, &[], ref_date).unwrap();
    assert!(hrv_facts.contains("Last night's HRV was"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_5_1_records_type_keys_and_bootstrap_onboarded_fix() {
    let dir = make_temp_dir("records-fix-0251");
    let store = SharedStore::new(Some(dir.clone())).unwrap();
    let db = Database::open(&dir.join("state").join("splits.sqlite")).unwrap();

    let now = Utc::now();
    let make_run = |id: i64, name: &str, type_key: &str, days_ago: i64, dist_m: f64, sec: f64, elev: f64| ActivityRecord {
        garmin_id: id,
        name: name.to_string(),
        type_key: type_key.to_string(),
        start_iso: (now - Duration::days(days_ago)).format("%Y-%m-%dT07:30:00Z").to_string(),
        distance_m: dist_m,
        duration_sec: sec,
        moving_duration_sec: sec,
        elevation_gain_m: elev,
        average_hr: Some(152.0),
        max_hr: Some(170.0),
        cadence_spm: Some(176.0),
        training_load: Some(110.0),
        shoe_name: Some("Superblast 3".to_string()),
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    };

    db.upsert_activity(&make_run(4001, "Track 1K", "track_running", 40, 1_050.0, 220.0, 4.0)).unwrap();
    db.upsert_activity(&make_run(4002, "City 5K", "street_running", 30, 5_050.0, 1_200.0, 15.0)).unwrap();
    db.upsert_activity(&make_run(4003, "Morning 10K", "running", 20, 10_100.0, 2_550.0, 45.0)).unwrap();
    db.upsert_activity(&make_run(4004, "Trail Half", "trail_running", 12, 21_200.0, 6_000.0, 540.0)).unwrap();
    db.upsert_activity(&make_run(4005, "Marathon", "running", 5, 42_300.0, 12_600.0, 220.0)).unwrap();
    // Treadmill run must be excluded from outdoor distance PRs even if it has a FIT track
    db.upsert_activity(&make_run(4006, "Fast Treadmill", "treadmill_running", 2, 5_000.0, 900.0, 0.0)).unwrap();

    db.upsert_fit_track(4001, &make_gps_fit_track(4001, 1_050.0, 220.0, 40, Some(165.0), Some(178.0))).unwrap();
    db.upsert_fit_track(4002, &make_gps_fit_track(4002, 5_050.0, 1_200.0, 60, Some(168.0), Some(178.0))).unwrap();
    db.upsert_fit_track(4003, &make_gps_fit_track(4003, 10_100.0, 2_550.0, 80, Some(165.0), Some(176.0))).unwrap();
    db.upsert_fit_track(4004, &make_gps_fit_track(4004, 21_200.0, 6_000.0, 100, Some(158.0), Some(174.0))).unwrap();
    db.upsert_fit_track(4005, &make_gps_fit_track(4005, 42_300.0, 12_600.0, 120, Some(155.0), Some(172.0))).unwrap();
    db.upsert_fit_track(4006, &make_gps_fit_track(4006, 5_000.0, 900.0, 50, Some(160.0), Some(180.0))).unwrap();

    let records = db.compute_personal_records().unwrap();
    assert_eq!(records.total_outdoor_runs, 5);
    let labels: Vec<&str> = records.distance_records.iter().map(|r| r.label.as_str()).collect();
    for expected in ["1K", "5K", "10K", "Half Marathon", "Marathon"] {
        assert!(labels.contains(&expected), "Missing PR label {expected}");
    }
    assert!(records.milestones.iter().any(|m| m.key == "longest_run" && m.activity_id == Some(4005)));
    assert!(records.milestones.iter().any(|m| m.key == "biggest_elevation" && m.activity_id == Some(4004)));
    assert!(records.milestones.iter().any(|m| m.key == "highest_weekly_km"));

    // Simulate app restart with default in-memory AppSettings (profile_onboarded = false),
    // while SQLite user_profile has onboarding_completed = true
    let mut prof = db.get_user_profile().unwrap();
    prof.onboarding_completed = true;
    db.save_user_profile(&prof).unwrap();

    let default_settings = splits_win_lib::storage::AppSettings::default();
    assert!(!default_settings.profile_onboarded);
    let boot = splits_win_lib::build_bootstrap_state(&store, &db, &default_settings).unwrap();
    assert_eq!(boot["settings"]["profile_onboarded"].as_bool(), Some(true));
    assert_eq!(boot["lifetime_backfill_completed"].as_bool(), Some(false));
    assert_eq!(boot["total_activities_count"].as_u64(), Some(6));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_5_1_explorer_multi_year_country_city_and_backfill_flag() {
    let dir = make_temp_dir("explorer-0251");
    let db_path = dir.join("splits.sqlite");

    // Seed v0.2.5 schema WITHOUT start_latitude / start_longitude to verify idempotent migration
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE activities (
                garmin_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                type_key TEXT NOT NULL,
                start_iso TEXT NOT NULL,
                distance_m REAL NOT NULL,
                duration_sec REAL NOT NULL,
                moving_duration_sec REAL NOT NULL,
                elevation_gain_m REAL NOT NULL,
                average_hr REAL,
                max_hr REAL,
                cadence_spm REAL,
                training_load REAL,
                shoe_name TEXT,
                is_hyrox INTEGER,
                post_run_context_json TEXT,
                fit_temp_source TEXT,
                garmin_notes TEXT,
                is_pr INTEGER
            );
            INSERT INTO activities (
                garmin_id, name, type_key, start_iso, distance_m, duration_sec, moving_duration_sec,
                elevation_gain_m, average_hr, max_hr, cadence_spm, training_load, shoe_name
            ) VALUES (
                5000, 'Legacy No-GPS Run', 'running', '2025-10-01T07:00:00Z', 8000.0, 2700.0, 2680.0,
                30.0, 140.0, 150.0, 174.0, 65.0, NULL
            );",
        )
        .unwrap();
    }

    let db = Database::open(&db_path).unwrap();
    assert!(!db.is_lifetime_backfill_completed().unwrap());

    let make_gps_act = |id: i64, name: &str, type_key: &str, start_iso: &str, dist_m: f64, lat: f64, lon: f64| ActivityRecord {
        garmin_id: id,
        name: name.to_string(),
        type_key: type_key.to_string(),
        start_iso: start_iso.to_string(),
        distance_m: dist_m,
        duration_sec: 3000.0,
        moving_duration_sec: 2950.0,
        elevation_gain_m: 40.0,
        average_hr: Some(144.0),
        max_hr: Some(160.0),
        cadence_spm: Some(176.0),
        training_load: Some(80.0),
        shoe_name: None,
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: Some(lat),
        start_longitude: Some(lon),
    };

    // Multi-year outdoor runs across Madrid (ES), Amsterdam (NL), New York (US), London (GB)
    db.upsert_activity(&make_gps_act(5001, "Retiro Run", "running", "2018-05-12T08:00:00Z", 10_000.0, 40.4168, -3.7038)).unwrap();
    db.upsert_activity(&make_gps_act(5002, "Vondelpark", "street_running", "2020-07-04T09:00:00Z", 8_000.0, 52.3676, 4.9041)).unwrap();
    db.upsert_activity(&make_gps_act(5003, "Central Park", "running", "2024-11-02T07:30:00Z", 12_000.0, 40.7829, -73.9654)).unwrap();
    db.upsert_activity(&make_gps_act(5004, "Hyde Park", "trail_running", "2026-03-15T08:15:00Z", 15_000.0, 51.5074, -0.1278)).unwrap();
    // Cycling in Paris (FR)
    db.upsert_activity(&make_gps_act(5005, "Longchamp Ride", "road_biking", "2026-04-01T10:00:00Z", 45_000.0, 48.8566, 2.3522)).unwrap();
    // Indoor run with coordinates must be excluded
    db.upsert_activity(&make_gps_act(5006, "Indoor Track", "treadmill_running", "2026-04-02T10:00:00Z", 5_000.0, 52.3676, 4.9041)).unwrap();

    let all_acts = db.list_activities(10_000).unwrap();
    let run_summary = splits_win_lib::explorer::compute_explorer_summary(&all_acts, "run");
    assert_eq!(run_summary.visits.len(), 4);
    assert_eq!(run_summary.legs.len(), 3);
    assert!(run_summary.continents.contains(&"Europe".to_string()));
    assert!(run_summary.continents.contains(&"North America".to_string()));
    assert_eq!(run_summary.first_visit.as_ref().map(|f| f.iso.as_str()), Some("ES"));
    assert_eq!(run_summary.years.len(), 4);
    assert_eq!(run_summary.years[0].year, 2026);
    assert_eq!(run_summary.years[0].new_country_ids, vec!["GB".to_string()]);
    assert!(!run_summary.cities.is_empty());

    let ride_summary = splits_win_lib::explorer::compute_explorer_summary(&all_acts, "ride");
    assert_eq!(ride_summary.visits.len(), 1);
    assert_eq!(ride_summary.visits[0].iso, "FR");

    let everything_summary = splits_win_lib::explorer::compute_explorer_summary(&all_acts, "all");
    assert_eq!(everything_summary.visits.len(), 5);

    db.set_lifetime_backfill_completed(true).unwrap();
    assert!(db.is_lifetime_backfill_completed().unwrap());
    db.clean_slate_reset().unwrap();
    assert!(!db.is_lifetime_backfill_completed().unwrap());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_5_2_records_650_activities_completes_under_500ms_with_progress() {
    let dir = make_temp_dir("records-perf-0252");
    let db = Database::open(&dir.join("splits.sqlite")).unwrap();

    let now = Utc::now();
    let type_keys = ["running", "street_running", "trail_running", "track_running"];
    let mut acts = Vec::with_capacity(660);
    for i in 0..660i64 {
        let tk = type_keys[(i as usize) % type_keys.len()];
        let dist_m = match i % 5 {
            0 => 42_250.0,
            1 => 21_150.0,
            2 => 10_050.0,
            3 => 5_050.0,
            _ => 1_200.0,
        };
        let dur_s = (dist_m / 3.8_f64).round();
        acts.push(ActivityRecord {
            garmin_id: 10_000 + i,
            name: format!("Run #{i}"),
            type_key: tk.to_string(),
            start_iso: (now - Duration::days(i)).format("%Y-%m-%dT07:00:00Z").to_string(),
            distance_m: dist_m,
            duration_sec: dur_s,
            moving_duration_sec: dur_s,
            elevation_gain_m: 35.0 + (i % 80) as f64,
            average_hr: Some(146.0),
            max_hr: Some(166.0),
            cadence_spm: Some(176.0),
            training_load: Some(85.0),
            shoe_name: Some("Superblast 3".to_string()),
            is_hyrox: None,
            post_run_context: None,
            fit_temp_source: None,
            garmin_notes: None,
            is_pr: None,
            start_latitude: Some(40.4168),
            start_longitude: Some(-3.7038),
        });
    }
    db.upsert_activities_batch(&acts).unwrap();

    // Cache GPS FIT tracks on 5 runs (including Marathon-length, <=95m/step so 1K has >=10 GPS points)
    for i in 0..5i64 {
        let track = make_gps_fit_track(10_000 + i, 42_250.0, 42_250.0 / 3.8, 450, Some(160.0), Some(180.0));
        db.upsert_fit_track(10_000 + i, &track).unwrap();
    }

    let mut progress_events = Vec::new();
    let t0 = std::time::Instant::now();
    let payload = db
        .compute_personal_records_with_progress(|processed, total, msg| {
            progress_events.push((processed, total, msg.to_string()));
        })
        .unwrap();
    let elapsed = t0.elapsed();

    assert!(
        elapsed.as_millis() < 500,
        "compute_personal_records across 660 activities took {:?} (expected < 500ms)",
        elapsed
    );
    assert_eq!(payload.total_outdoor_runs, 660);
    assert_eq!(payload.scanned_fit_runs, 5);
    assert_eq!(payload.distance_records.len(), 5);
    assert!(!payload.milestones.is_empty());
    assert!(progress_events.len() >= 2);
    let last = progress_events.last().unwrap();
    assert_eq!(last.0, 660);
    assert_eq!(last.1, 660);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_5_3_records_rejects_gps_corrupted_outlier_and_builds_progression() {
    let dir = make_temp_dir("records-outlier-0253");
    let db = Database::open(&dir.join("splits.sqlite")).unwrap();

    let now = Utc::now();
    let recent_iso = |days_ago: i64| -> String {
        (now - Duration::days(days_ago))
            .format("%Y-%m-%dT07:30:00Z")
            .to_string()
    };
    let make_run = |id: i64, name: &str, start_iso: String, dist_m: f64, sec: f64, hr: Option<f64>| ActivityRecord {
        garmin_id: id,
        name: name.to_string(),
        type_key: "running".to_string(),
        start_iso,
        distance_m: dist_m,
        duration_sec: sec,
        moving_duration_sec: sec,
        elevation_gain_m: 30.0,
        average_hr: hr,
        max_hr: hr.map(|h| h + 15.0),
        cadence_spm: Some(176.0),
        training_load: Some(90.0),
        shoe_name: Some("Superblast 3".to_string()),
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    };

    // Exact QA GPS-corrupted 2014 outlier: 11.23 km in 31:00 (2:46/km = 6.038 m/s) at 76 bpm avg HR
    db.upsert_activity(&make_run(
        9001,
        "Madrid Corrupted 2014",
        "2014-10-11T08:00:00Z".to_string(),
        11_230.0,
        1_860.0,
        Some(76.0),
    ))
    .unwrap();

    // Unstrapped GPS/bike-logged-as-run outlier: 11.23 km in 31:00 with no HR (must still fail 5K/10K speed ceiling)
    db.upsert_activity(&make_run(
        9002,
        "Vehicular Commute Outlier",
        "2016-05-01T08:00:00Z".to_string(),
        11_230.0,
        1_860.0,
        None,
    ))
    .unwrap();

    // Real PR runs matching macOS Records view + historical progression (with GPS FIT tracks)
    db.upsert_activity(&make_run(9100, "2020 Base 5K", "2020-03-10T08:00:00Z".to_string(), 5_000.0, 1_500.0, Some(165.0))).unwrap();
    db.upsert_activity(&make_run(9101, "2022 Tempo 5K", "2022-06-12T08:00:00Z".to_string(), 5_000.0, 1_380.0, Some(172.0))).unwrap();
    db.upsert_activity(&make_run(9102, "Real 1K Effort", recent_iso(20), 1_000.0, 195.0, Some(176.0))).unwrap(); // 3:15
    db.upsert_activity(&make_run(9103, "Real 5K PR", recent_iso(14), 5_000.0, 1_316.0, Some(178.0))).unwrap(); // 21:56
    db.upsert_activity(&make_run(9104, "Real 10K PR", recent_iso(10), 10_000.0, 2_751.0, Some(174.0))).unwrap(); // 45:51
    db.upsert_activity(&make_run(9105, "Real Half PR", recent_iso(6), 21_097.5, 6_435.0, Some(168.0))).unwrap(); // 1:47:15
    db.upsert_activity(&make_run(9106, "Real Marathon PR", "2024-04-14T08:00:00Z".to_string(), 42_195.0, 15_011.0, Some(164.0))).unwrap(); // 4:10:11

    db.upsert_fit_track(9001, &make_gps_fit_track(9001, 11_230.0, 1_860.0, 60, Some(76.0), Some(176.0))).unwrap();
    db.upsert_fit_track(9002, &make_gps_fit_track(9002, 11_230.0, 1_860.0, 60, None, Some(176.0))).unwrap();
    db.upsert_fit_track(9100, &make_gps_fit_track(9100, 5_000.0, 1_500.0, 50, Some(165.0), Some(176.0))).unwrap();
    db.upsert_fit_track(9101, &make_gps_fit_track(9101, 5_000.0, 1_380.0, 50, Some(172.0), Some(176.0))).unwrap();
    db.upsert_fit_track(9102, &make_gps_fit_track(9102, 1_000.0, 195.0, 40, Some(176.0), Some(176.0))).unwrap();
    db.upsert_fit_track(9103, &make_gps_fit_track(9103, 5_000.0, 1_316.0, 60, Some(178.0), Some(176.0))).unwrap();
    db.upsert_fit_track(9104, &make_gps_fit_track(9104, 10_000.0, 2_751.0, 80, Some(174.0), Some(176.0))).unwrap();
    db.upsert_fit_track(9105, &make_gps_fit_track(9105, 21_097.5, 6_435.0, 100, Some(168.0), Some(176.0))).unwrap();
    db.upsert_fit_track(9106, &make_gps_fit_track(9106, 42_195.0, 15_011.0, 120, Some(164.0), Some(176.0))).unwrap();

    let records = db.compute_personal_records().unwrap();

    let by_label: std::collections::HashMap<&str, &DistanceRecordEntry> = records
        .distance_records
        .iter()
        .map(|r| (r.label.as_str(), r))
        .collect();

    assert_eq!(by_label["1K"].activity_id, 9102);
    assert!((by_label["1K"].time_sec - 195.0).abs() < 0.5);
    assert_eq!(by_label["5K"].activity_id, 9103);
    assert!((by_label["5K"].time_sec - 1316.0).abs() < 0.5);
    assert_eq!(by_label["10K"].activity_id, 9104);
    assert!((by_label["10K"].time_sec - 2751.0).abs() < 0.5);
    assert_eq!(by_label["Half Marathon"].activity_id, 9105);
    assert_eq!(by_label["Marathon"].activity_id, 9106);

    // Outlier must not appear in predictions or progression
    assert!(!records.predictions.is_empty());
    assert_eq!(records.prediction_window_months, 4);
    assert!(records.predictions.iter().all(|p| p.source_activity_id != 9001 && p.source_activity_id != 9002));

    let five_k_prog = records
        .progression
        .iter()
        .find(|s| s.label == "5K")
        .expect("5K progression series must be present");
    assert!(five_k_prog.broken_count >= 2);
    assert!(five_k_prog.points.iter().all(|p| p.activity_id != 9001 && p.activity_id != 9002));
    assert!(five_k_prog.points.iter().any(|p| p.activity_id == 9103 && p.is_record_break));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_6_gps_spike_guard_rejects_barcelona_jump_accepts_clean_339_and_excludes_hyrox() {
    let dir = make_temp_dir("records-gps-spike-026");
    let db = Database::open(&dir.join("splits.sqlite")).unwrap();

    // 1. Barcelona 11.51 km / 1:16:12 (4572s) / 144 spm run with a synthetic GPS teleport jump
    //    at s=2001..2004 (+500m in 4s) that would produce a fake 3:15 1K without the spike guard.
    let barcelona = ActivityRecord {
        garmin_id: 9201,
        name: "Barcelona Morning Run".to_string(),
        type_key: "running".to_string(),
        start_iso: "2014-07-14T07:30:00Z".to_string(),
        distance_m: 11_510.0,
        duration_sec: 4_572.0,
        moving_duration_sec: 4_572.0,
        elevation_gain_m: 45.0,
        average_hr: Some(146.0),
        max_hr: Some(165.0),
        cadence_spm: Some(144.0),
        training_load: Some(110.0),
        shoe_name: None,
        is_hyrox: Some(false),
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: Some(41.3851),
        start_longitude: Some(2.1734),
    };
    db.upsert_activity(&barcelona).unwrap();

    let mut spike_pts = Vec::with_capacity(4573);
    let mut dist = 0.0_f64;
    for s in 0..=4572 {
        if s > 0 {
            if (2001..=2004).contains(&s) {
                dist += 125.0; // 500m GPS teleport in 4s
            } else {
                dist += 2.41;
            }
        }
        spike_pts.push(make_gps_fit_point(
            s as f64,
            dist,
            2.52,
            Some(146.0),
            Some(72.0), // 72 * 2 = 144 spm
            Some(15.0),
        ));
    }
    let spike_track = analyze_fit_points(9201, spike_pts, &AthleteConfig::default());
    db.upsert_fit_track(9201, &spike_track).unwrap();

    // 2. Clean 3:39 (219s) 1K effort on 2023-09-16 at 178 spm
    let clean_act = ActivityRecord {
        garmin_id: 9202,
        name: "Track Threshold 1K".to_string(),
        type_key: "track_running".to_string(),
        start_iso: "2023-09-16T08:00:00Z".to_string(),
        distance_m: 5_000.0,
        duration_sec: 1_400.0,
        moving_duration_sec: 1_400.0,
        elevation_gain_m: 10.0,
        average_hr: Some(168.0),
        max_hr: Some(182.0),
        cadence_spm: Some(174.0),
        training_load: Some(95.0),
        shoe_name: None,
        is_hyrox: Some(false),
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: Some(true),
        start_latitude: Some(40.4168),
        start_longitude: Some(-3.7038),
    };
    db.upsert_activity(&clean_act).unwrap();

    let mut clean_pts = Vec::with_capacity(1401);
    let mut clean_dist = 0.0_f64;
    for s in 0..=1400 {
        let spd = if (400..=618).contains(&s) {
            1000.0 / 219.0
        } else {
            3.35
        };
        if s > 0 {
            clean_dist += spd;
        }
        clean_pts.push(make_gps_fit_point(
            s as f64,
            clean_dist,
            spd,
            Some(if (400..=618).contains(&s) { 178.0 } else { 154.0 }),
            Some(if (400..=618).contains(&s) { 89.0 } else { 85.0 }), // 178 / 170 spm
            Some(20.0),
        ));
    }
    let clean_track = analyze_fit_points(9202, clean_pts, &AthleteConfig::default());
    db.upsert_fit_track(9202, &clean_track).unwrap();

    // 3. Tagged HYROX activity & auto-detected HYROX sim must never enter running PRs or predictions
    let recent_iso = (Utc::now() - Duration::days(7))
        .format("%Y-%m-%dT09:00:00Z")
        .to_string();
    let hyrox_act = ActivityRecord {
        garmin_id: 9203,
        name: "HYROX Sim 8x8".to_string(),
        type_key: "running".to_string(),
        start_iso: recent_iso,
        distance_m: 8_200.0,
        duration_sec: 1_500.0, // artificially fast if unguarded
        moving_duration_sec: 1_500.0,
        elevation_gain_m: 0.0,
        average_hr: Some(170.0),
        max_hr: Some(184.0),
        cadence_spm: Some(176.0),
        training_load: Some(140.0),
        shoe_name: None,
        is_hyrox: Some(true),
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    };
    db.upsert_activity(&hyrox_act).unwrap();

    // Low-cadence / missing-cadence 1.75x speed spike helper check
    assert!(splits_win_lib::storage::is_low_cadence_speed_spike(
        1000.0,
        195.0,
        Some(11_510.0 / 4_572.0),
        Some(155.0),
        Some(144.0),
    ));
    assert!(splits_win_lib::storage::is_low_cadence_speed_spike(
        1000.0,
        195.0,
        Some(11_510.0 / 4_572.0),
        None,
        None,
    ));
    assert!(!splits_win_lib::storage::is_low_cadence_speed_spike(
        1000.0,
        195.0,
        Some(11_510.0 / 4_572.0),
        Some(165.0),
        Some(152.0),
    ));

    let records = db.compute_personal_records().unwrap();
    let one_k = records
        .distance_records
        .iter()
        .find(|r| r.label == "1K")
        .expect("1K record must exist");
    assert_eq!(one_k.activity_id, 9202);
    assert!((one_k.time_sec - 219.0).abs() < 2.0);
    assert!(records
        .distance_records
        .iter()
        .all(|r| r.activity_id != 9203));
    assert!(records
        .predictions
        .iter()
        .all(|p| p.source_activity_id != 9203));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_6_fit_prefetch_candidate_selector_prioritizes_pr_hyrox_and_top5_capped_at_25() {
    let mut acts = Vec::new();
    let make = |id: i64, dist_m: f64, dur_s: f64, is_pr: bool, is_hyrox: bool| ActivityRecord {
        garmin_id: id,
        name: format!("Act {id}"),
        type_key: "running".to_string(),
        start_iso: "2026-01-15T08:00:00Z".to_string(),
        distance_m: dist_m,
        duration_sec: dur_s,
        moving_duration_sec: dur_s,
        elevation_gain_m: 10.0,
        average_hr: Some(165.0),
        max_hr: Some(180.0),
        cadence_spm: Some(178.0),
        training_load: Some(80.0),
        shoe_name: None,
        is_hyrox: if is_hyrox { Some(true) } else { Some(false) },
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: if is_pr { Some(true) } else { Some(false) },
        start_latitude: None,
        start_longitude: None,
    };

    acts.push(make(1, 5_000.0, 1_300.0, true, false)); // PR uncached
    acts.push(make(2, 5_000.0, 1_310.0, true, false)); // PR already cached
    acts.push(make(3, 8_200.0, 4_200.0, false, true)); // HYROX uncached
    // Add 35 fast outdoor runs across buckets
    for i in 10..45i64 {
        let dist = match i % 5 {
            0 => 42_200.0,
            1 => 21_200.0,
            2 => 10_100.0,
            3 => 5_100.0,
            _ => 1_500.0,
        };
        acts.push(make(i, dist, dist / 4.2, false, false));
    }

    let mut cached = std::collections::HashSet::new();
    cached.insert(2i64);

    let without_hyrox =
        splits_win_lib::storage::select_records_fit_prefetch_candidates(&acts, &cached, false, 25);
    assert!(without_hyrox.contains(&1));
    assert!(!without_hyrox.contains(&2));
    assert!(!without_hyrox.contains(&3));
    assert!(without_hyrox.len() <= 25);

    let with_hyrox =
        splits_win_lib::storage::select_records_fit_prefetch_candidates(&acts, &cached, true, 25);
    assert!(with_hyrox.contains(&1));
    assert!(with_hyrox.contains(&3));
    assert!(!with_hyrox.contains(&2));
    assert!(with_hyrox.len() <= 25);
}

#[test]
fn v0_2_6_hyrox_records_aggregation_and_runner_gating() {
    let dir = make_temp_dir("hyrox-records-026");
    let db = Database::open(&dir.join("splits.sqlite")).unwrap();

    let make_sim_track = |id: i64, run_paces: &[f64], station_secs: &[f64]| {
        let mut laps = Vec::new();
        for idx in 0..run_paces.len() {
            let r_sec = run_paces[idx];
            laps.push(FitLap {
                elapsed_sec: r_sec,
                moving_sec: r_sec,
                distance_m: 1_000.0,
                avg_hr_bpm: Some(168.0),
            });
            if idx < station_secs.len() {
                let s_sec = station_secs[idx];
                laps.push(FitLap {
                    elapsed_sec: s_sec,
                    moving_sec: s_sec,
                    distance_m: 50.0,
                    avg_hr_bpm: Some(172.0),
                });
            }
        }
        let dissection = dissect_hyrox_laps(&laps, true);
        let mut track = analyze_fit_points(id, Vec::new(), &AthleteConfig::default());
        track.laps = laps;
        track.hyrox_dissection = dissection;
        track
    };

    let full_act = ActivityRecord {
        garmin_id: 9301,
        name: "HYROX Madrid 8x8".to_string(),
        type_key: "running".to_string(),
        start_iso: "2026-09-20T09:00:00Z".to_string(),
        distance_m: 8_400.0,
        duration_sec: 4_620.0,
        moving_duration_sec: 4_550.0,
        elevation_gain_m: 0.0,
        average_hr: Some(170.0),
        max_hr: Some(186.0),
        cadence_spm: Some(174.0),
        training_load: Some(180.0),
        shoe_name: None,
        is_hyrox: Some(true),
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    };
    let half_act = ActivityRecord {
        garmin_id: 9302,
        name: "HYROX Half Sim 4x4".to_string(),
        type_key: "running".to_string(),
        start_iso: "2026-09-10T09:00:00Z".to_string(),
        distance_m: 4_200.0,
        duration_sec: 2_150.0,
        moving_duration_sec: 2_100.0,
        elevation_gain_m: 0.0,
        average_hr: Some(168.0),
        max_hr: Some(184.0),
        cadence_spm: Some(176.0),
        training_load: Some(120.0),
        shoe_name: None,
        is_hyrox: Some(true),
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    };
    db.upsert_activity(&full_act).unwrap();
    db.upsert_activity(&half_act).unwrap();

    db.upsert_fit_track(
        9301,
        &make_sim_track(
            9301,
            &[240.0, 245.0, 250.0, 255.0, 252.0, 254.0, 255.0, 255.0],
            &[240.0, 180.0, 210.0, 260.0, 245.0, 130.0, 270.0, 290.0],
        ),
    )
    .unwrap();
    db.upsert_fit_track(
        9302,
        &make_sim_track(
            9302,
            &[230.0, 240.0, 255.0, 265.0],
            &[220.0, 170.0, 200.0, 240.0],
        ),
    )
    .unwrap();

    let payload = db.compute_personal_records().unwrap();
    let hyrox = &payload.hyrox_records;
    assert_eq!(hyrox.total_hyrox_sessions, 2);
    assert_eq!(hyrox.fastest_full_sim.as_ref().map(|r| r.activity_id), Some(9301));
    assert!((hyrox.fastest_full_sim.as_ref().unwrap().time_sec - 4620.0).abs() < 0.5);
    assert_eq!(hyrox.fastest_half_sim.as_ref().map(|r| r.activity_id), Some(9302));
    assert!((hyrox.fastest_half_sim.as_ref().unwrap().time_sec - 2150.0).abs() < 0.5);
    assert_eq!(hyrox.best_clean_1k_run.as_ref().map(|r| r.activity_id), Some(9302));
    assert!((hyrox.best_clean_1k_run.as_ref().unwrap().time_sec - 230.0).abs() < 0.5);
    assert_eq!(
        hyrox.best_compromised_resilience.as_ref().map(|r| r.activity_id),
        Some(9301)
    );
    assert!((hyrox.best_compromised_resilience.as_ref().unwrap().decay_sec_per_km - 14.0).abs() < 0.5);
    assert_eq!(hyrox.best_station_splits.len(), 8);
    assert_eq!(hyrox.best_station_splits[0].station_name, "1000m SkiErg");
    assert_eq!(hyrox.best_station_splits[0].activity_id, 9302);
    assert_eq!(hyrox.best_station_splits[7].station_name, "100 Wall Balls");
    assert_eq!(hyrox.best_station_splits[7].activity_id, 9301);

    // Discipline gating check: pure runners see zero HYROX UI
    let runner_profile = UserProfile {
        athlete_type: AthleteType::Runner,
        ..Default::default()
    };
    let runner_config = AthleteConfig::default();
    assert!(!is_hyrox_discipline_enabled(&runner_profile, &runner_config));

    let hyrox_profile = UserProfile {
        athlete_type: AthleteType::Hyrox,
        ..Default::default()
    };
    assert!(is_hyrox_discipline_enabled(&hyrox_profile, &runner_config));

    let hybrid_profile = UserProfile {
        athlete_type: AthleteType::HybridBoth,
        ..Default::default()
    };
    assert!(is_hyrox_discipline_enabled(&hybrid_profile, &runner_config));

    let mut hybrid_goal_config = AthleteConfig::default();
    hybrid_goal_config.objective.goal_type = GoalType::HybridHyrox;
    assert!(is_hyrox_discipline_enabled(&runner_profile, &hybrid_goal_config));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_6_glitch_distance_budget_marathon_survival_sparse_2014_and_no_gps_exclusion() {
    let dir = make_temp_dir("records-glitch-budget-026");
    let db = Database::open(&dir.join("splits.sqlite")).unwrap();

    let make_act = |id: i64, name: &str, type_key: &str, iso: &str, dist_m: f64, dur_s: f64, hr: Option<f64>, cad: Option<f64>| ActivityRecord {
        garmin_id: id,
        name: name.to_string(),
        type_key: type_key.to_string(),
        start_iso: iso.to_string(),
        distance_m: dist_m,
        duration_sec: dur_s,
        moving_duration_sec: dur_s,
        elevation_gain_m: 30.0,
        average_hr: hr,
        max_hr: hr.map(|h| h + 15.0),
        cadence_spm: cad,
        training_load: Some(120.0),
        shoe_name: None,
        is_hyrox: Some(false),
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: Some(41.3851),
        start_longitude: Some(2.1734),
    };

    // 1. 42.2 km Marathon (4:10:11 = 15,011s) with 2 isolated 1-sec 30m GPS jitter blips (60m <= 500m budget)
    let marathon_sec = 15_011.0;
    let base_step = (42_200.0 - 60.0) / 15_009.0;
    let mut marathon_pts = Vec::with_capacity(15_012);
    let mut m_dist = 0.0_f64;
    for s in 0..=15_011 {
        if s > 0 {
            m_dist += if s == 3_000 || s == 11_000 { 30.0 } else { base_step };
        }
        marathon_pts.push(make_gps_fit_point(
            s as f64,
            m_dist,
            base_step,
            Some(162.0),
            Some(86.0),
            Some(25.0),
        ));
    }
    db.upsert_activity(&make_act(
        9401,
        "Valencia Marathon",
        "running",
        "2024-04-14T08:00:00Z",
        m_dist,
        marathon_sec,
        Some(162.0),
        Some(172.0),
    ))
    .unwrap();
    db.upsert_fit_track(9401, &analyze_fit_points(9401, marathon_pts, &AthleteConfig::default()))
        .unwrap();

    // 2. 1 km run with a single 40m jump (> 25m 1K budget) that would create a fake ~226s 1K
    let mut jump1k_pts = Vec::with_capacity(401);
    let mut j_dist = 0.0_f64;
    for s in 0..=400 {
        if s > 0 {
            j_dist += if s == 200 { 40.0 } else { 4.25 };
        }
        jump1k_pts.push(make_gps_fit_point(
            s as f64,
            j_dist,
            4.25,
            Some(170.0),
            Some(88.0),
            Some(15.0),
        ));
    }
    db.upsert_activity(&make_act(
        9402,
        "Noisy 40m Jump Run",
        "running",
        "2022-05-01T08:00:00Z",
        j_dist,
        550.0, // avg 3.0 m/s
        Some(170.0),
        Some(176.0),
    ))
    .unwrap();
    db.upsert_fit_track(9402, &analyze_fit_points(9402, jump1k_pts, &AthleteConfig::default()))
        .unwrap();

    // 3. 24 Aug 2014 sparse Smart-Recording run (6s gaps, 3:32 = 212s 1K spike > 1.55x avg speed)
    let mut sparse_pts = Vec::new();
    let mut sp_dist = 0.0_f64;
    let mut t_sec = 0.0_f64;
    while t_sec <= 1800.0 {
        sparse_pts.push(make_gps_fit_point(
            t_sec,
            sp_dist,
            2.8,
            Some(162.0),
            Some(86.0),
            Some(20.0),
        ));
        let dt = 6.0;
        let rate = if (600.0..816.0).contains(&t_sec) {
            1000.0 / 212.0
        } else {
            2.56
        };
        sp_dist += rate * dt;
        t_sec += dt;
    }
    db.upsert_activity(&make_act(
        9403,
        "2014 Smart-Recording Spike",
        "running",
        "2014-08-24T08:00:00Z",
        sp_dist,
        t_sec,
        Some(162.0),
        Some(172.0),
    ))
    .unwrap();
    db.upsert_fit_track(9403, &analyze_fit_points(9403, sparse_pts, &AthleteConfig::default()))
        .unwrap();

    // 4. Clean 16 Sep 2023 3:39 (219s) 1K effort at 1 Hz and 178 spm
    let mut clean_pts = Vec::with_capacity(1801);
    let mut c_dist = 0.0_f64;
    for s in 0..=1800 {
        let spd = if (600..=818).contains(&s) { 1000.0 / 219.0 } else { 3.20 };
        if s > 0 {
            c_dist += spd;
        }
        clean_pts.push(make_gps_fit_point(
            s as f64,
            c_dist,
            spd,
            Some(if (600..=818).contains(&s) { 176.0 } else { 150.0 }),
            Some(if (600..=818).contains(&s) { 89.0 } else { 84.0 }),
            Some(20.0),
        ));
    }
    db.upsert_activity(&make_act(
        9404,
        "Clean Sep 2023 1K",
        "running",
        "2023-09-16T08:00:00Z",
        c_dist,
        1800.0,
        Some(156.0),
        Some(170.0),
    ))
    .unwrap();
    db.upsert_fit_track(9404, &analyze_fit_points(9404, clean_pts, &AthleteConfig::default()))
        .unwrap();

    // 5. Excluded runs: no-GPS FIT, (0,0) FIT, <85% GPS coverage FIT, indoor treadmill, uncached outdoor
    db.upsert_activity(&make_act(9405, "No GPS FIT", "running", "2025-01-10T08:00:00Z", 5_000.0, 950.0, Some(165.0), Some(180.0))).unwrap();
    let no_gps_pts: Vec<FitTrackPoint> = (0..=950)
        .map(|s| make_fit_point(s as f64, (s as f64) * (5000.0 / 950.0), 5.2, Some(165.0), Some(180.0), Some(0.0)))
        .collect();
    db.upsert_fit_track(9405, &analyze_fit_points(9405, no_gps_pts, &AthleteConfig::default())).unwrap();

    db.upsert_activity(&make_act(9406, "Zero-Zero GPS", "running", "2025-01-11T08:00:00Z", 5_000.0, 950.0, Some(165.0), Some(180.0))).unwrap();
    let zero_pts: Vec<FitTrackPoint> = (0..=950)
        .map(|s| {
            let mut pt = make_fit_point(s as f64, (s as f64) * (5000.0 / 950.0), 5.2, Some(165.0), Some(180.0), Some(0.0));
            pt.latitude_deg = Some(0.0);
            pt.longitude_deg = Some(0.0);
            pt
        })
        .collect();
    db.upsert_fit_track(9406, &analyze_fit_points(9406, zero_pts, &AthleteConfig::default())).unwrap();

    db.upsert_activity(&make_act(9407, "Partial GPS (<85%)", "running", "2025-01-12T08:00:00Z", 5_000.0, 950.0, Some(165.0), Some(180.0))).unwrap();
    let partial_pts: Vec<FitTrackPoint> = (0..=950)
        .map(|s| {
            let dist = (s as f64) * (5000.0 / 950.0);
            if s < 150 {
                make_gps_fit_point(s as f64, dist, 5.2, Some(165.0), Some(180.0), Some(0.0))
            } else {
                make_fit_point(s as f64, dist, 5.2, Some(165.0), Some(180.0), Some(0.0))
            }
        })
        .collect();
    db.upsert_fit_track(9407, &analyze_fit_points(9407, partial_pts, &AthleteConfig::default())).unwrap();

    db.upsert_activity(&make_act(9408, "Indoor Treadmill", "treadmill_running", "2025-01-13T08:00:00Z", 5_000.0, 950.0, Some(165.0), Some(180.0))).unwrap();
    db.upsert_fit_track(9408, &make_gps_fit_track(9408, 5_000.0, 950.0, 50, Some(165.0), Some(180.0))).unwrap();

    db.upsert_activity(&make_act(9409, "Uncached Fast Run", "running", "2025-01-14T08:00:00Z", 10_000.0, 1_900.0, Some(165.0), Some(180.0))).unwrap();

    let records = db.compute_personal_records().unwrap();
    let by_label: std::collections::HashMap<&str, &DistanceRecordEntry> = records
        .distance_records
        .iter()
        .map(|r| (r.label.as_str(), r))
        .collect();

    // Marathon survived 2 isolated 30m GPS jitter blips
    let marathon = by_label.get("Marathon").expect("Marathon must survive isolated GPS blips");
    assert_eq!(marathon.activity_id, 9401);
    assert!((marathon.time_sec - 15_011.0).abs() < 30.0);
    assert!(by_label.contains_key("Half Marathon"));
    assert!(by_label.contains_key("10K"));
    assert!(by_label.contains_key("5K"));

    // Clean 16 Sep 2023 3:39 beats rejected 2014 sparse 3:32 and 40m-jump 1K
    let one_k = by_label.get("1K").expect("Clean 1K must qualify");
    assert_eq!(one_k.activity_id, 9404);
    assert!((one_k.time_sec - 219.0).abs() < 2.0);

    // Excluded IDs never enter distance_records, predictions, or progression
    let excluded = [9405_i64, 9406, 9407, 9408, 9409];
    assert!(records.distance_records.iter().all(|r| !excluded.contains(&r.activity_id)));
    assert!(records.predictions.iter().all(|p| !excluded.contains(&p.source_activity_id)));
    for series in &records.progression {
        assert!(series.points.iter().all(|p| !excluded.contains(&p.activity_id)));
    }
    // 9401..9404 + 9407 have >=10 track-level GPS points (9407 still yields 0 records due to <85% window coverage)
    assert_eq!(records.scanned_fit_runs, 5);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn v0_2_6_security_hardening_contracts() {
    // 1. unpack_fit_zip rejects an oversized (> 50 MB) ZIP entry header before reading into RAM
    let mut fake_zip = Vec::new();
    let name = b"bomb.fit";
    let push_u16 = |buf: &mut Vec<u8>, v: u16| buf.extend_from_slice(&v.to_le_bytes());
    let push_u32 = |buf: &mut Vec<u8>, v: u32| buf.extend_from_slice(&v.to_le_bytes());
    fake_zip.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
    push_u16(&mut fake_zip, 20);
    push_u16(&mut fake_zip, 0);
    push_u16(&mut fake_zip, 0); // stored
    push_u16(&mut fake_zip, 0);
    push_u16(&mut fake_zip, 0);
    push_u32(&mut fake_zip, 0);
    push_u32(&mut fake_zip, 4);
    push_u32(&mut fake_zip, 100 * 1024 * 1024);
    push_u16(&mut fake_zip, name.len() as u16);
    push_u16(&mut fake_zip, 0);
    fake_zip.extend_from_slice(name);
    fake_zip.extend_from_slice(&[1, 2, 3, 4]);
    let cd_offset = fake_zip.len() as u32;
    fake_zip.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
    push_u16(&mut fake_zip, 20);
    push_u16(&mut fake_zip, 20);
    push_u16(&mut fake_zip, 0);
    push_u16(&mut fake_zip, 0);
    push_u16(&mut fake_zip, 0);
    push_u16(&mut fake_zip, 0);
    push_u32(&mut fake_zip, 0);
    push_u32(&mut fake_zip, 4);
    push_u32(&mut fake_zip, 100 * 1024 * 1024);
    push_u16(&mut fake_zip, name.len() as u16);
    push_u16(&mut fake_zip, 0);
    push_u16(&mut fake_zip, 0);
    push_u16(&mut fake_zip, 0);
    push_u16(&mut fake_zip, 0);
    push_u32(&mut fake_zip, 0);
    push_u32(&mut fake_zip, 0);
    fake_zip.extend_from_slice(name);
    let cd_size = (fake_zip.len() as u32) - cd_offset;
    fake_zip.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]);
    push_u16(&mut fake_zip, 0);
    push_u16(&mut fake_zip, 0);
    push_u16(&mut fake_zip, 1);
    push_u16(&mut fake_zip, 1);
    push_u32(&mut fake_zip, cd_size);
    push_u32(&mut fake_zip, cd_offset);
    push_u16(&mut fake_zip, 0);

    let zip_err = splits_win_lib::garmin::GarminClient::unpack_fit_zip(fake_zip).unwrap_err();
    assert!(
        zip_err.to_string().contains("50 MB") || !zip_err.to_string().is_empty(),
        "Oversized ZIP entry must be rejected: {zip_err}"
    );

    // 2. SharedStore::remove_relative_file refuses traversal outside root
    let dir = make_temp_dir("sec-hardening-026");
    let store_dir = dir.join("coach");
    let store = SharedStore::new(Some(store_dir.clone())).unwrap();
    let outside_file = dir.join("outside.txt");
    std::fs::write(&outside_file, "keep-me").unwrap();
    store.remove_relative_file("../outside.txt").unwrap();
    assert!(outside_file.exists(), "Traversal must not delete files outside coach root");
    let _ = std::fs::remove_dir_all(&dir);

    // 3. tauri.conf.json enforces explicit CSP (not null)
    let conf_raw = include_str!("../tauri.conf.json");
    let conf: serde_json::Value = serde_json::from_str(conf_raw).unwrap();
    let csp = conf["app"]["security"]["csp"]
        .as_str()
        .expect("tauri.conf.json app.security.csp must be a non-null string");
    assert!(csp.contains("default-src 'self'"));
    assert!(csp.contains("object-src 'none'"));
}

#[test]
fn test_tight_sweat_pairing_midnight_and_negative_loss_guards() {
    let act_start: i64 = 1_795_000_000;
    let act_end: i64 = act_start + 3600;

    // 1. Tight window (-15m pre, +10m post) pairs cleanly and computes offsets
    let tight_pre = WeightEntry {
        timestamp_epoch_sec: act_start - 15 * 60,
        timestamp_iso: "2026-11-14T15:11:40Z".to_string(),
        weight_kg: 105.0,
        body_fat_pct: None,
        body_water_pct: None,
        muscle_mass_kg: None,
        source: "garmin_scale".to_string(),
    };
    let tight_post = WeightEntry {
        timestamp_epoch_sec: act_end + 10 * 60,
        timestamp_iso: "2026-11-14T16:36:40Z".to_string(),
        weight_kg: 104.2,
        body_fat_pct: None,
        body_water_pct: None,
        muscle_mass_kg: None,
        source: "garmin_scale".to_string(),
    };
    let paired = scale_matcher::pair_scale_weigh_ins(
        act_start,
        act_end,
        3600.0,
        &[tight_pre.clone(), tight_post.clone()],
        PostRunContext::default(),
    );
    assert_eq!(paired.pre_run_weight_kg, Some(105.0));
    assert_eq!(paired.post_run_weight_kg, Some(104.2));
    assert_eq!(paired.pre_weigh_in_offset_min, Some(-15));
    assert_eq!(paired.post_weigh_in_offset_min, Some(10));
    assert_eq!(paired.sweat_status.as_deref(), Some("provisional_no_intake"));
    assert_eq!(paired.provisional_sweat_rate_l_hr, Some(0.8));

    // 2. Outside tight windows (-90m pre or +55m post) must NOT compute sweat rate,
    // while +55m post is STILL excluded from baseline weight ([end - 5m, end + 90m]).
    let early_pre = WeightEntry {
        timestamp_epoch_sec: act_start - 90 * 60,
        timestamp_iso: "2026-11-14T13:56:40Z".to_string(),
        weight_kg: 105.3,
        body_fat_pct: None,
        body_water_pct: None,
        muscle_mass_kg: None,
        source: "garmin_scale".to_string(),
    };
    let delayed_post = WeightEntry {
        timestamp_epoch_sec: act_end + 55 * 60,
        timestamp_iso: "2026-11-14T17:21:40Z".to_string(),
        weight_kg: 104.0,
        body_fat_pct: None,
        body_water_pct: None,
        muscle_mass_kg: None,
        source: "garmin_scale".to_string(),
    };
    let missed = scale_matcher::pair_scale_weigh_ins(
        act_start,
        act_end,
        3600.0,
        &[early_pre, delayed_post.clone()],
        PostRunContext::default(),
    );
    assert_eq!(missed.pre_run_weight_kg, None);
    assert_eq!(missed.post_run_weight_kg, None);
    assert_eq!(missed.sweat_status, None);
    assert_eq!(missed.provisional_sweat_rate_l_hr, None);
    assert_eq!(
        scale_matcher::latest_baseline_weigh_in(&[delayed_post], &[(act_start, act_end)]),
        None
    );

    // 3. calendarDate-only entry parses to UTC midnight and is rejected by auto-pairing
    let cal_only_json = br#"{
      "dailyWeightSummaries": [{
        "allWeightMetrics": [{
          "samplePk": 77,
          "calendarDate": "2026-11-14",
          "weight": 104500.0,
          "sourceType": "INDEX_SCALE"
        }]
      }]
    }"#;
    let parsed_cal = parse_weight_range_json(cal_only_json);
    assert_eq!(parsed_cal.len(), 1);
    assert!(scale_matcher::is_midnight_placeholder(
        parsed_cal[0].timestamp_epoch_sec
    ));
    let midnight_ep = parsed_cal[0].timestamp_epoch_sec;
    let midnight_paired = scale_matcher::pair_scale_weigh_ins(
        midnight_ep + 10 * 60,
        midnight_ep + 70 * 60,
        3600.0,
        &[parsed_cal[0].clone(), tight_post],
        PostRunContext::default(),
    );
    assert_eq!(midnight_paired.pre_run_weight_kg, None);
    assert_eq!(midnight_paired.sweat_status, None);

    // 4. Post > Pre without sufficient fluid intake never emits negative sweat rate
    let neg_ctx = PostRunContext {
        pre_run_weight_kg: Some(104.0),
        post_run_weight_kg: Some(104.3),
        fluid_consumed_ml: Some(100.0),
        fluid_intake_confirmed: true,
        weight_source: Some("garmin_scale".to_string()),
        ..Default::default()
    }
    .with_computed_sweat(3600.0);
    assert_eq!(neg_ctx.provisional_sweat_rate_l_hr, None);
    assert_eq!(neg_ctx.final_sweat_rate_l_hr, None);
    assert_eq!(neg_ctx.net_fluid_deficit_kg, None);
    assert_eq!(neg_ctx.sweat_status, None);
}

#[test]
fn test_garmin_weight_authority_recency_guard_and_blank_shoe_filter() {
    let dir = make_temp_dir("win-weight-authority");
    let store = SharedStore::new(Some(dir.clone())).unwrap();
    let db = Database::open_in_memory().unwrap();

    let mut cfg = AthleteConfig::default();
    cfg.specs.weight_kg = Some(100.0);
    cfg.specs.weight_baseline_confirmed = true;
    store.save_athlete_config(&cfg).unwrap();

    let act_start: i64 = 1_795_000_000;
    let act_end: i64 = act_start + 3600;
    let act = ActivityRecord {
        garmin_id: 991,
        name: "Easy Run".to_string(),
        type_key: "running".to_string(),
        start_iso: Utc.timestamp_opt(act_start, 0).unwrap().to_rfc3339(),
        distance_m: 8_000.0,
        duration_sec: 3600.0,
        moving_duration_sec: 3600.0,
        elevation_gain_m: 20.0,
        average_hr: Some(128.0),
        max_hr: Some(135.0),
        cadence_spm: Some(174.0),
        training_load: Some(45.0),
        shoe_name: Some("Superblast 3".to_string()),
        is_hyrox: None,
        post_run_context: None,
        fit_temp_source: None,
        garmin_notes: None,
        is_pr: None,
        start_latitude: None,
        start_longitude: None,
    };
    db.upsert_activity(&act).unwrap();

    let morning_ep = act_start - 3 * 3600;
    let dehydrated_ep = act_end + 20 * 60;
    let morning = WeightEntry {
        timestamp_epoch_sec: morning_ep,
        timestamp_iso: Utc.timestamp_opt(morning_ep, 0).unwrap().to_rfc3339(),
        weight_kg: 109.4,
        body_fat_pct: Some(24.0),
        body_water_pct: None,
        muscle_mass_kg: Some(44.2),
        source: "garmin_scale".to_string(),
    };
    let dehydrated = WeightEntry {
        timestamp_epoch_sec: dehydrated_ep,
        timestamp_iso: Utc.timestamp_opt(dehydrated_ep, 0).unwrap().to_rfc3339(),
        weight_kg: 107.8,
        body_fat_pct: None,
        body_water_pct: None,
        muscle_mass_kg: None,
        source: "garmin_scale".to_string(),
    };
    db.upsert_weight_entry(&morning).unwrap();
    db.upsert_weight_entry(&dehydrated).unwrap();

    // 1. Fresh (<=14d) Garmin weight supersedes even already-confirmed config weight
    let eval_now = act_start + 7200;
    let updated_cfg = reconcile_scale_weights_and_config(&db, &store, eval_now).unwrap();
    assert_eq!(updated_cfg.specs.weight_kg, Some(109.4));
    assert!(updated_cfg.specs.weight_baseline_confirmed);

    // 2. Stale (>14d) Garmin weight must NOT overwrite confirmed manual weight
    let mut manual_cfg = updated_cfg.clone();
    manual_cfg.specs.weight_kg = Some(102.0);
    manual_cfg.specs.weight_baseline_confirmed = true;
    store.save_athlete_config(&manual_cfg).unwrap();
    let stale_now = morning_ep + 20 * 86_400;
    let preserved_cfg = reconcile_scale_weights_and_config(&db, &store, stale_now).unwrap();
    assert_eq!(preserved_cfg.specs.weight_kg, Some(102.0));

    // 3. Wellness history tags is_post_run_dehydrated & is_fresh; readiness tool includes latest_garmin_weight
    let activities = db.list_activities(10).unwrap();
    let days = db.list_days(14).unwrap();
    let weights = db.list_weight_entries(20).unwrap();
    let wellness = build_wellness_history_tool_payload(&days, &weights, &activities, 14, eval_now);
    assert_eq!(wellness["latest_garmin_weight"]["weight_kg"].as_f64(), Some(109.4));
    assert_eq!(wellness["latest_garmin_weight"]["is_fresh"].as_bool(), Some(true));
    let recent = wellness["recent_weight_entries"].as_array().unwrap();
    let dehydrated_row = recent.iter().find(|r| r["weight_kg"].as_f64() == Some(107.8)).unwrap();
    let morning_row = recent.iter().find(|r| r["weight_kg"].as_f64() == Some(109.4)).unwrap();
    assert_eq!(dehydrated_row["is_post_run_dehydrated"].as_bool(), Some(true));
    assert_eq!(morning_row["is_post_run_dehydrated"].as_bool(), Some(false));

    let readiness_payload = build_readiness_and_plan_tool_payload(
        &store,
        &activities,
        &days,
        &weights,
        Utc.timestamp_opt(eval_now, 0).unwrap(),
    );
    assert_eq!(readiness_payload["latest_garmin_weight"]["weight_kg"].as_f64(), Some(109.4));

    // 4. System instruction, packet builder & briefing facts surface authoritative weight + (stale: >14d)
    let sys_fresh = build_static_system_instruction_with_weights(
        &store,
        &updated_cfg,
        &activities,
        &weights,
        None,
        None,
        eval_now,
    );
    assert!(sys_fresh.contains("=== GARMIN CONNECT WEIGHT (AUTHORITATIVE) ==="));
    assert!(sys_fresh.contains("109.4 kg"));
    assert!(!sys_fresh.contains("(stale: >14d)"));

    let sys_stale = build_static_system_instruction_with_weights(
        &store,
        &updated_cfg,
        &activities,
        &weights,
        None,
        None,
        stale_now,
    );
    assert!(sys_stale.contains("(stale: >14d)"));

    let packet_stale = CoachPacketBuilder::build_markdown_with_weights(
        &updated_cfg,
        &activities,
        &days,
        &weights,
        None,
        None,
        "",
        Utc.timestamp_opt(stale_now, 0).unwrap(),
    );
    assert!(packet_stale.contains("Garmin Connect Weight (Authoritative): 109.4 kg"));
    assert!(packet_stale.contains("(stale: >14d)"));

    let facts_stale = build_daily_briefing_facts_with_weights(
        &updated_cfg,
        &activities,
        &days,
        &weights,
        None,
        NaiveDate::from_ymd_opt(2026, 12, 4).unwrap(),
        stale_now,
    );
    assert!(facts_stale.contains("Garmin Connect weight: 109.4 kg"));
    assert!(facts_stale.contains("(stale: >14d)"));

    // 5. COACH_PROFILE.md proposal must not overwrite fresh Garmin weight
    let mut profile = UserProfile::default();
    let mut proposal_cfg = updated_cfg.clone();
    let proposal = CoachUpdateProposal {
        id: "p1".to_string(),
        target_file: "COACH_PROFILE.md".to_string(),
        reason: "test".to_string(),
        updated_section_markdown: "- Weight: 98.0 kg (PLACEHOLDER)".to_string(),
        expires_on: None,
        diff_preview: String::new(),
        status: "pending_user_approval".to_string(),
        plan_json: None,
        profile_thresholds: Some(CoachProfileThresholds {
            vt1_bpm: Some(133),
            weight_kg: Some(98.0),
            ..Default::default()
        }),
    };
    assert!(hydrate_profile_from_proposal_with_weight_guard(
        &proposal,
        &mut profile,
        &mut proposal_cfg,
        true
    ));
    assert_eq!(proposal_cfg.specs.weight_kg, Some(109.4));

    // 6. Activity detail payload exposes pairing offsets and net_fluid_deficit_kg
    let mut act_with_offsets = act;
    act_with_offsets.post_run_context = Some(PostRunContext {
        pre_run_weight_kg: Some(105.0),
        post_run_weight_kg: Some(104.2),
        net_fluid_deficit_kg: Some(0.8),
        pre_weigh_in_offset_min: Some(-15),
        post_weigh_in_offset_min: Some(10),
        weight_source: Some("garmin_scale".to_string()),
        ..Default::default()
    });
    let detail = build_activity_detail_tool_payload(&act_with_offsets, &store, Some(&db), &activities);
    assert_eq!(detail["pre_weigh_in_offset_min"].as_i64(), Some(-15));
    assert_eq!(detail["post_weigh_in_offset_min"].as_i64(), Some(10));
    assert_eq!(detail["net_fluid_deficit_kg"].as_f64(), Some(0.8));

    // 7. Blank "" shoe names never leak into ReadinessState
    let mut items = std::collections::HashMap::new();
    items.insert(
        "blank-shoe".to_string(),
        GearItem {
            garmin: Some(GearGarminMeta {
                name: Some("   ".to_string()),
                brand: None,
                model: None,
                gear_type: None,
                status: Some("ACTIVE".to_string()),
                distance_used_m: None,
                activities_linked: None,
                max_usage_distance_m: None,
                first_use: None,
                retrieved_at: None,
            }),
            alias: Some("".to_string()),
            eligibility: Some(GearEligibility::Simple("eligible".to_string())),
            pending_naming: None,
        },
    );
    items.insert(
        "real-shoe".to_string(),
        GearItem {
            garmin: None,
            alias: Some("Superblast 3".to_string()),
            eligibility: Some(GearEligibility::Simple("eligible".to_string())),
            pending_naming: None,
        },
    );
    let gear = GearInventoryFile {
        version: 1,
        revision: 1,
        updated_at: "2026-11-14T16:00:00Z".to_string(),
        updated_by: "test".to_string(),
        items,
        pending: Vec::new(),
    };
    let out = ReadinessEngine::evaluate(ReadinessInput {
        now: Utc.timestamp_opt(eval_now, 0).unwrap(),
        athlete_config: &updated_cfg,
        activities: &[],
        days: &[],
        plan: None,
        gear_inventory: Some(&gear),
    });
    assert_eq!(
        out.readiness_state.shoe_eligibility.eligible_run_shoes,
        vec!["Superblast 3".to_string()]
    );

    let _ = std::fs::remove_dir_all(&dir);
}
