use chrono::{Duration, TimeZone, Utc};
use splits_win_lib::coach::{
    analyze_training_history, append_forensic_section, append_training_ledger_entry,
    apply_coach_proposal, apply_post_run_context_tool_update, compact_history_markdown,
    fetch_1hz_window_from_track, format_user_profile_context, replacing_section,
    slice_fit_track_by_km, starter_coach_profile_markdown, starter_training_plan_markdown,
    CoachPacketBuilder,
};
use splits_win_lib::forensic_extractor::extract_forensic_telemetry;
use splits_win_lib::garmin::{analyze_fit_points, dissect_hyrox_laps, parse_weight_range_json};
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
            is_hyrox: None,
            post_run_context: None,
            fit_temp_source: None,
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
    };
    db.upsert_activity(&act).unwrap();
    let tool_res =
        apply_post_run_context_tool_update(&db, &act, Some(300.0), Some(true)).unwrap();
    assert_eq!(tool_res["sweat_status"].as_str(), Some("final_verified"));
    assert_eq!(tool_res["final_sweat_rate_l_hr"].as_f64(), Some(1.5));
    assert_eq!(tool_res["temp_source"].as_str(), Some("user_confirmed_tempe"));
}
