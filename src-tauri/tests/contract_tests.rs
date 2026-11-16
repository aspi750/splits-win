use chrono::{Duration, TimeZone, Utc};
use splits_win_lib::coach::{
    analyze_training_history, apply_coach_proposal, compact_history_markdown, replacing_section,
    slice_fit_track_by_km, starter_coach_profile_markdown, starter_training_plan_markdown,
    CoachPacketBuilder,
};
use splits_win_lib::garmin::analyze_fit_points;
use splits_win_lib::models::*;
use splits_win_lib::readiness::{ReadinessEngine, ReadinessInput};
use splits_win_lib::scheduler::{
    is_within_catch_up_window, should_catch_up_briefing, should_send_morning_notification,
};
use splits_win_lib::storage::{Database, SharedStore};

fn make_temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("splits-win-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
        FitTrackPoint { timestamp_sec: 0.0, distance_m: 0.0, speed_mps: 3.0, pace_sec_per_km: Some(333.3), heart_rate_bpm: Some(120.0), cadence_spm: Some(174.0), altitude_m: Some(10.0) },
        FitTrackPoint { timestamp_sec: 160.0, distance_m: 500.0, speed_mps: 3.0, pace_sec_per_km: Some(333.3), heart_rate_bpm: Some(120.0), cadence_spm: Some(174.0), altitude_m: Some(12.0) },
        FitTrackPoint { timestamp_sec: 340.0, distance_m: 1000.0, speed_mps: 3.0, pace_sec_per_km: Some(333.3), heart_rate_bpm: Some(120.0), cadence_spm: Some(174.0), altitude_m: Some(15.0) },
        FitTrackPoint { timestamp_sec: 520.0, distance_m: 1500.0, speed_mps: 3.0, pace_sec_per_km: Some(333.3), heart_rate_bpm: Some(150.0), cadence_spm: Some(172.0), altitude_m: Some(15.0) },
        FitTrackPoint { timestamp_sec: 700.0, distance_m: 2000.0, speed_mps: 3.0, pace_sec_per_km: Some(333.3), heart_rate_bpm: Some(150.0), cadence_spm: Some(172.0), altitude_m: Some(16.0) },
        FitTrackPoint { timestamp_sec: 880.0, distance_m: 2500.0, speed_mps: 3.0, pace_sec_per_km: Some(333.3), heart_rate_bpm: Some(150.0), cadence_spm: Some(172.0), altitude_m: Some(16.0) },
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
        FitTrackPoint { timestamp_sec: 0.0, distance_m: 0.0, speed_mps: 3.0, pace_sec_per_km: Some(333.3), heart_rate_bpm: Some(125.0), cadence_spm: Some(174.0), altitude_m: Some(10.0) },
        FitTrackPoint { timestamp_sec: 200.0, distance_m: 600.0, speed_mps: 3.0, pace_sec_per_km: Some(333.3), heart_rate_bpm: Some(130.0), cadence_spm: Some(175.0), altitude_m: Some(11.0) },
        FitTrackPoint { timestamp_sec: 400.0, distance_m: 1200.0, speed_mps: 3.0, pace_sec_per_km: Some(333.3), heart_rate_bpm: Some(135.0), cadence_spm: Some(176.0), altitude_m: Some(12.0) },
        FitTrackPoint { timestamp_sec: 600.0, distance_m: 1800.0, speed_mps: 3.0, pace_sec_per_km: Some(333.3), heart_rate_bpm: Some(138.0), cadence_spm: Some(176.0), altitude_m: Some(14.0) },
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
