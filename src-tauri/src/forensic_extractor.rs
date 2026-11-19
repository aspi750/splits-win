use crate::models::{FitLap, FitTrackPoint};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricDistribution {
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub first_half_mean: Option<f64>,
    pub second_half_mean: Option<f64>,
    pub drift: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectionalHalfStats {
    pub sample_count: usize,
    pub mean_hr: Option<f64>,
    pub mean_pace_sec_per_km: Option<f64>,
    pub mean_power_w: Option<f64>,
    pub mean_cadence_spm: Option<f64>,
    pub mean_gct_ms: Option<f64>,
    pub mean_left_stance_balance_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectionalVectorSplit {
    pub turnaround_distance_m: f64,
    pub turnaround_elapsed_s: f64,
    pub max_displacement_m: f64,
    pub outbound: DirectionalHalfStats,
    pub inbound: DirectionalHalfStats,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VelocityBandStats {
    pub band: String,
    pub sample_count: usize,
    pub mean_gct_ms: Option<f64>,
    pub mean_cadence_spm: Option<f64>,
    pub mean_vert_osc_mm: Option<f64>,
    pub mean_vert_ratio_pct: Option<f64>,
    pub mean_step_length_mm: Option<f64>,
    pub mean_power_w: Option<f64>,
    pub mean_left_stance_balance_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LapForensicDistribution {
    pub lap_index: usize,
    pub distance_m: f64,
    pub duration_s: f64,
    pub mean_hr: Option<f64>,
    pub mean_pace_sec_per_km: Option<f64>,
    pub mean_power_w: Option<f64>,
    pub mean_cadence_spm: Option<f64>,
    pub mean_gct_ms: Option<f64>,
    pub mean_vert_osc_mm: Option<f64>,
    pub mean_stride_mm: Option<f64>,
    pub mean_stance_balance_l_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForensicMicroEvent {
    pub kind: String,
    pub elapsed_s: f64,
    pub duration_s: Option<f64>,
    pub distance_m: Option<f64>,
    pub summary: String,
    pub pre_hr: Option<f64>,
    pub post_hr: Option<f64>,
    pub hr_bpm: Option<f64>,
    pub pace_min_per_km: Option<String>,
    pub temp_c: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForensicTelemetryReport {
    pub sample_count: usize,
    pub hr: Option<MetricDistribution>,
    pub pace_sec_per_km: Option<MetricDistribution>,
    pub power_w: Option<MetricDistribution>,
    pub cadence_spm: Option<MetricDistribution>,
    pub gct_ms: Option<MetricDistribution>,
    pub vertical_oscillation_mm: Option<MetricDistribution>,
    pub stride_length_mm: Option<MetricDistribution>,
    pub stance_balance_l_pct: Option<MetricDistribution>,
    pub left_stance_over_52_8_pct: Option<f64>,
    pub respiration_brpm: Option<MetricDistribution>,
    pub impact_load_total: Option<f64>,
    pub pw_hr_decoupling_pct: Option<f64>,
    pub pa_hr_decoupling_pct: Option<f64>,
    pub walk_seconds: f64,
    pub run_seconds: f64,
    pub walk_segments: usize,
    pub run_segments: usize,
    pub vt1_breach_seconds: f64,
    pub vt2_breach_seconds: f64,
    pub z5_breach_seconds: f64,
    pub directional_vectors: Option<DirectionalVectorSplit>,
    pub velocity_bands: Vec<VelocityBandStats>,
    pub laps: Vec<LapForensicDistribution>,
    pub micro_events: Vec<ForensicMicroEvent>,
    #[serde(default = "crate::models::default_temp_source")]
    pub temp_source: String,
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn fmt_pace(sec_per_km: f64) -> String {
    let total = sec_per_km.round().max(0.0) as i32;
    format!("{}:{:02}/km", total / 60, total % 60)
}

fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

pub fn extract_forensic_telemetry(
    points: &[FitTrackPoint],
    laps: &[FitLap],
) -> ForensicTelemetryReport {
    extract_forensic_telemetry_with_source(points, laps, "watch_internal")
}

pub fn extract_forensic_telemetry_with_source(
    points: &[FitTrackPoint],
    laps: &[FitLap],
    temp_source: &str,
) -> ForensicTelemetryReport {
    if points.is_empty() {
        return ForensicTelemetryReport {
            sample_count: 0,
            hr: None,
            pace_sec_per_km: None,
            power_w: None,
            cadence_spm: None,
            gct_ms: None,
            vertical_oscillation_mm: None,
            stride_length_mm: None,
            stance_balance_l_pct: None,
            left_stance_over_52_8_pct: None,
            respiration_brpm: None,
            impact_load_total: None,
            pw_hr_decoupling_pct: None,
            pa_hr_decoupling_pct: None,
            walk_seconds: 0.0,
            run_seconds: 0.0,
            walk_segments: 0,
            run_segments: 0,
            vt1_breach_seconds: 0.0,
            vt2_breach_seconds: 0.0,
            z5_breach_seconds: 0.0,
            directional_vectors: None,
            velocity_bands: Vec::new(),
            laps: Vec::new(),
            micro_events: Vec::new(),
            temp_source: temp_source.to_string(),
        };
    }

    let dist = |extractor: &dyn Fn(&FitTrackPoint) -> Option<f64>| -> Option<MetricDistribution> {
        let indexed: Vec<(usize, f64)> = points
            .iter()
            .enumerate()
            .filter_map(|(i, p)| extractor(p).map(|v| (i, v)))
            .collect();
        if indexed.is_empty() {
            return None;
        }
        let mean = indexed.iter().map(|(_, v)| *v).sum::<f64>() / indexed.len() as f64;
        let min_v = indexed
            .iter()
            .map(|(_, v)| *v)
            .fold(f64::INFINITY, f64::min);
        let max_v = indexed
            .iter()
            .map(|(_, v)| *v)
            .fold(f64::NEG_INFINITY, f64::max);
        let mid = points.len() / 2;
        let first: Vec<f64> = indexed.iter().filter(|(i, _)| *i < mid).map(|(_, v)| *v).collect();
        let second: Vec<f64> = indexed.iter().filter(|(i, _)| *i >= mid).map(|(_, v)| *v).collect();
        let m1 = if first.is_empty() {
            None
        } else {
            Some(first.iter().sum::<f64>() / first.len() as f64)
        };
        let m2 = if second.is_empty() {
            None
        } else {
            Some(second.iter().sum::<f64>() / second.len() as f64)
        };
        let drift = match (m1, m2) {
            (Some(a), Some(b)) => Some(r2(b - a)),
            _ => None,
        };
        Some(MetricDistribution {
            mean: r2(mean),
            min: r2(min_v),
            max: r2(max_v),
            first_half_mean: m1.map(r2),
            second_half_mean: m2.map(r2),
            drift,
        })
    };

    let hr_dist = dist(&|p| p.heart_rate_bpm);
    let pace_dist = dist(&|p| if p.speed_mps > 0.3 { Some(1000.0 / p.speed_mps) } else { None });
    let power_dist = dist(&|p| p.power_w);
    let cadence_dist = dist(&|p| p.cadence_spm);
    let gct_dist = dist(&|p| p.gct_ms);
    let vert_osc_dist = dist(&|p| p.vertical_oscillation_mm);
    let stride_dist = dist(&|p| p.step_length_mm);
    let stance_dist = dist(&|p| p.stance_balance_left_pct);
    let resp_dist = dist(&|p| p.respiration_brpm);

    let impact_vals: Vec<f64> = points.iter().filter_map(|p| p.impact_load).collect();
    let impact_load_total = if impact_vals.is_empty() {
        None
    } else {
        Some(r1(impact_vals.iter().sum()))
    };

    let stance_vals: Vec<f64> = points.iter().filter_map(|p| p.stance_balance_left_pct).collect();
    let left_stance_over_52_8_pct = if stance_vals.is_empty() {
        None
    } else {
        let over = stance_vals.iter().filter(|&&v| v > 52.8).count();
        Some(r1((over as f64 / stance_vals.len() as f64) * 100.0))
    };

    // Active running samples (speed >= 1.5 m/s and cadence >= 120 spm)
    let active_run_pts: Vec<&FitTrackPoint> = points
        .iter()
        .filter(|p| p.speed_mps >= 1.5 && p.cadence_spm.map_or(true, |c| c >= 120.0))
        .collect();
    let decoupling_src: Vec<&FitTrackPoint> = if active_run_pts.len() >= 30 {
        active_run_pts.clone()
    } else {
        points.iter().collect()
    };

    let pw_hr_decoupling_pct = {
        let mid = decoupling_src.len() / 2;
        let p1: Vec<f64> = decoupling_src[..mid]
            .iter()
            .filter_map(|p| match (p.power_w, p.heart_rate_bpm) {
                (Some(pw), Some(hr)) if pw > 0.0 && hr > 40.0 => Some(pw / hr),
                _ => None,
            })
            .collect();
        let p2: Vec<f64> = decoupling_src[mid..]
            .iter()
            .filter_map(|p| match (p.power_w, p.heart_rate_bpm) {
                (Some(pw), Some(hr)) if pw > 0.0 && hr > 40.0 => Some(pw / hr),
                _ => None,
            })
            .collect();
        if p1.len() >= 15 && p2.len() >= 15 {
            let ef1 = p1.iter().sum::<f64>() / p1.len() as f64;
            let ef2 = p2.iter().sum::<f64>() / p2.len() as f64;
            if ef1 > 0.0 {
                Some(r2(((ef1 - ef2) / ef1) * 100.0))
            } else {
                None
            }
        } else {
            None
        }
    };

    let pa_hr_decoupling_pct = {
        let mid = decoupling_src.len() / 2;
        let s1: Vec<f64> = decoupling_src[..mid]
            .iter()
            .filter_map(|p| match (p.speed_mps > 0.3, p.heart_rate_bpm) {
                (true, Some(hr)) if hr > 40.0 => Some(p.speed_mps / hr),
                _ => None,
            })
            .collect();
        let s2: Vec<f64> = decoupling_src[mid..]
            .iter()
            .filter_map(|p| match (p.speed_mps > 0.3, p.heart_rate_bpm) {
                (true, Some(hr)) if hr > 40.0 => Some(p.speed_mps / hr),
                _ => None,
            })
            .collect();
        if s1.len() >= 15 && s2.len() >= 15 {
            let ef1 = s1.iter().sum::<f64>() / s1.len() as f64;
            let ef2 = s2.iter().sum::<f64>() / s2.len() as f64;
            if ef1 > 0.0 {
                Some(r2(((ef1 - ef2) / ef1) * 100.0))
            } else {
                None
            }
        } else {
            None
        }
    };

    // Outbound vs Inbound directional vectors
    let directional_vectors = {
        let start_coord = points
            .iter()
            .find_map(|p| match (p.latitude_deg, p.longitude_deg) {
                (Some(la), Some(lo)) => Some((la, lo)),
                _ => None,
            });
        if let Some((s_lat, s_lon)) = start_coord {
            let mut max_disp = 0.0_f64;
            let mut turnaround_idx: Option<usize> = None;
            for (i, p) in points.iter().enumerate() {
                if let (Some(lat), Some(lon)) = (p.latitude_deg, p.longitude_deg) {
                    let d = haversine_m(s_lat, s_lon, lat, lon);
                    if d > max_disp {
                        max_disp = d;
                        turnaround_idx = Some(i);
                    }
                }
            }
            if let Some(t_idx) = turnaround_idx.filter(|&i| max_disp >= 150.0 && i > 5 && i + 5 < points.len()) {
                let half_stats = |slice: &[FitTrackPoint]| -> DirectionalHalfStats {
                    let mean_of = |f: &dyn Fn(&FitTrackPoint) -> Option<f64>| -> Option<f64> {
                        let vals: Vec<f64> = slice.iter().filter_map(f).collect();
                        if vals.is_empty() {
                            None
                        } else {
                            Some(r2(vals.iter().sum::<f64>() / vals.len() as f64))
                        }
                    };
                    DirectionalHalfStats {
                        sample_count: slice.len(),
                        mean_hr: mean_of(&|p| p.heart_rate_bpm),
                        mean_pace_sec_per_km: mean_of(&|p| if p.speed_mps > 0.3 { Some(1000.0 / p.speed_mps) } else { None }),
                        mean_power_w: mean_of(&|p| p.power_w),
                        mean_cadence_spm: mean_of(&|p| p.cadence_spm),
                        mean_gct_ms: mean_of(&|p| p.gct_ms),
                        mean_left_stance_balance_pct: mean_of(&|p| p.stance_balance_left_pct),
                    }
                };
                Some(DirectionalVectorSplit {
                    turnaround_distance_m: r1(points[t_idx].distance_m),
                    turnaround_elapsed_s: r1(points[t_idx].timestamp_sec),
                    max_displacement_m: r1(max_disp),
                    outbound: half_stats(&points[..t_idx]),
                    inbound: half_stats(&points[t_idx..]),
                })
            } else {
                None
            }
        } else {
            None
        }
    };

    // Velocity-Band Biomechanics on Active Running samples
    let mut velocity_bands = Vec::new();
    let bands: [(&str, fn(f64) -> bool); 3] = [
        ("<5:45/km", |pace| pace < 345.0),
        ("5:45-6:30/km", |pace| (345.0..=390.0).contains(&pace)),
        (">6:30/km", |pace| pace > 390.0),
    ];
    for (label, pred) in bands {
        let matched: Vec<&FitTrackPoint> = active_run_pts
            .iter()
            .copied()
            .filter(|p| p.speed_mps >= 1.5 && pred(1000.0 / p.speed_mps))
            .collect();
        if matched.is_empty() {
            continue;
        }
        let b_mean = |f: &dyn Fn(&FitTrackPoint) -> Option<f64>| -> Option<f64> {
            let vals: Vec<f64> = matched.iter().filter_map(|p| f(p)).collect();
            if vals.is_empty() {
                None
            } else {
                Some(r2(vals.iter().sum::<f64>() / vals.len() as f64))
            }
        };
        velocity_bands.push(VelocityBandStats {
            band: label.to_string(),
            sample_count: matched.len(),
            mean_gct_ms: b_mean(&|p| p.gct_ms),
            mean_cadence_spm: b_mean(&|p| p.cadence_spm),
            mean_vert_osc_mm: b_mean(&|p| p.vertical_oscillation_mm),
            mean_vert_ratio_pct: b_mean(&|p| p.vertical_ratio_pct),
            mean_step_length_mm: b_mean(&|p| p.step_length_mm),
            mean_power_w: b_mean(&|p| p.power_w),
            mean_left_stance_balance_pct: b_mean(&|p| p.stance_balance_left_pct),
        });
    }

    // Regime separation + VT1/VT2/Z5 breaches + Micro-events
    let mut walk_sec = 0.0;
    let mut run_sec = 0.0;
    let mut walk_segs = 0usize;
    let mut run_segs = 0usize;
    let mut current_mode: Option<&str> = None;

    let mut vt1_sec = 0.0;
    let mut vt2_sec = 0.0;
    let mut z5_sec = 0.0;

    for i in 1..points.len() {
        let dt = (points[i].timestamp_sec - points[i - 1].timestamp_sec).clamp(0.0, 5.0);
        let cad = points[i].cadence_spm.unwrap_or(0.0);
        if cad > 0.0 && cad < 120.0 {
            walk_sec += dt;
            if current_mode != Some("walk") {
                walk_segs += 1;
            }
            current_mode = Some("walk");
        } else if cad >= 120.0 {
            run_sec += dt;
            if current_mode != Some("run") {
                run_segs += 1;
            }
            current_mode = Some("run");
        }
        let hr = points[i].heart_rate_bpm.unwrap_or(0.0);
        if hr > 138.0 { vt1_sec += dt; }
        if hr > 162.0 { vt2_sec += dt; }
        if hr >= 175.0 { z5_sec += dt; }
    }

    let mut micro_events = Vec::new();

    // 1. Stops / pauses (< 0.3 m/s for >= 5s)
    let mut stop_start: Option<usize> = None;
    for i in 0..points.len() {
        if points[i].speed_mps < 0.3 {
            if stop_start.is_none() {
                stop_start = Some(i);
            }
        } else if let Some(s_idx) = stop_start.take() {
            let dur = points[i - 1].timestamp_sec - points[s_idx].timestamp_sec;
            if dur >= 5.0 {
                let dist_m = points[s_idx].distance_m;
                let pre = points[s_idx].heart_rate_bpm;
                let post = points[i].heart_rate_bpm;
                micro_events.push(ForensicMicroEvent {
                    kind: "stop_pause".to_string(),
                    elapsed_s: r1(points[s_idx].timestamp_sec),
                    duration_s: Some(r1(dur)),
                    distance_m: Some(r1(dist_m)),
                    summary: format!(
                        "Stopped {}s at {:.0}m (HR {} -> {} bpm)",
                        dur.round() as i32,
                        dist_m,
                        pre.map_or("–".to_string(), |v| format!("{}", v.round() as i32)),
                        post.map_or("–".to_string(), |v| format!("{}", v.round() as i32)),
                    ),
                    pre_hr: pre,
                    post_hr: post,
                    hr_bpm: pre,
                    pace_min_per_km: None,
                    temp_c: points[s_idx].temperature_c,
                });
            }
        }
    }

    // 2. Tempe thermal swings (>= 2.0 C)
    let mut min_t: Option<(f64, f64)> = None;
    let mut max_t: Option<(f64, f64)> = None;
    for p in points {
        if let Some(t) = p.temperature_c {
            if min_t.map_or(true, |(_, mt)| t < mt) {
                min_t = Some((p.timestamp_sec, t));
            }
            if max_t.map_or(true, |(_, mt)| t > mt) {
                max_t = Some((p.timestamp_sec, t));
            }
        }
    }
    if let (Some((_, t_lo)), Some((ts_hi, t_hi))) = (min_t, max_t) {
        if t_hi - t_lo >= 2.0 {
            micro_events.push(ForensicMicroEvent {
                kind: "thermal_swing".to_string(),
                elapsed_s: r1(ts_hi),
                duration_s: None,
                distance_m: None,
                summary: format!("Tempe swing {:.1}°C ({t_lo:.1}°C -> {t_hi:.1}°C)", t_hi - t_lo),
                pre_hr: None,
                post_hr: None,
                hr_bpm: None,
                pace_min_per_km: None,
                temp_c: Some(t_hi),
            });
        }
    }

    // 3. HR Zone breaches (>= 145 bpm sustained >= 5s)
    let mut breach_start: Option<usize> = None;
    let mut breach_emitted = false;
    for i in 0..points.len() {
        let hr = points[i].heart_rate_bpm.unwrap_or(0.0);
        if hr >= 145.0 {
            if breach_start.is_none() {
                breach_start = Some(i);
                breach_emitted = false;
            }
            if let Some(b_idx) = breach_start {
                if !breach_emitted && (points[i].timestamp_sec - points[b_idx].timestamp_sec) >= 5.0 {
                    breach_emitted = true;
                    let pace_str = if points[i].speed_mps > 0.3 {
                        Some(fmt_pace(1000.0 / points[i].speed_mps))
                    } else {
                        None
                    };
                    micro_events.push(ForensicMicroEvent {
                        kind: "hr_zone_breach".to_string(),
                        elapsed_s: r1(points[b_idx].timestamp_sec),
                        duration_s: None,
                        distance_m: Some(r1(points[b_idx].distance_m)),
                        summary: format!(
                            "HR >= 145 bpm ({} bpm at {})",
                            hr.round() as i32,
                            pace_str.as_deref().unwrap_or("–")
                        ),
                        pre_hr: None,
                        post_hr: None,
                        hr_bpm: Some(hr),
                        pace_min_per_km: pace_str,
                        temp_c: points[i].temperature_c,
                    });
                }
            }
        } else {
            breach_start = None;
            breach_emitted = false;
        }
    }

    // Per-lap forensic distributions
    let mut lap_rows = Vec::new();
    let mut cursor_dist = 0.0;
    for (idx, lap) in laps.iter().enumerate() {
        let end_dist = cursor_dist + lap.distance_m;
        let slice: Vec<&FitTrackPoint> = points
            .iter()
            .filter(|p| p.distance_m >= cursor_dist && p.distance_m <= end_dist.max(cursor_dist + 1.0))
            .collect();
        cursor_dist = end_dist;
        let l_mean = |f: &dyn Fn(&FitTrackPoint) -> Option<f64>| -> Option<f64> {
            let vals: Vec<f64> = slice.iter().filter_map(|p| f(p)).collect();
            if vals.is_empty() {
                None
            } else {
                Some(r2(vals.iter().sum::<f64>() / vals.len() as f64))
            }
        };
        let dur = if lap.moving_sec > 0.0 { lap.moving_sec } else { lap.elapsed_sec };
        lap_rows.push(LapForensicDistribution {
            lap_index: idx + 1,
            distance_m: lap.distance_m,
            duration_s: dur,
            mean_hr: l_mean(&|p| p.heart_rate_bpm).or(lap.avg_hr_bpm),
            mean_pace_sec_per_km: if lap.distance_m > 0.0 && dur > 0.0 {
                Some(r1(dur / (lap.distance_m / 1000.0)))
            } else {
                None
            },
            mean_power_w: l_mean(&|p| p.power_w),
            mean_cadence_spm: l_mean(&|p| p.cadence_spm),
            mean_gct_ms: l_mean(&|p| p.gct_ms),
            mean_vert_osc_mm: l_mean(&|p| p.vertical_oscillation_mm),
            mean_stride_mm: l_mean(&|p| p.step_length_mm),
            mean_stance_balance_l_pct: l_mean(&|p| p.stance_balance_left_pct),
        });
    }

    ForensicTelemetryReport {
        sample_count: points.len(),
        hr: hr_dist,
        pace_sec_per_km: pace_dist,
        power_w: power_dist,
        cadence_spm: cadence_dist,
        gct_ms: gct_dist,
        vertical_oscillation_mm: vert_osc_dist,
        stride_length_mm: stride_dist,
        stance_balance_l_pct: stance_dist,
        left_stance_over_52_8_pct,
        respiration_brpm: resp_dist,
        impact_load_total,
        pw_hr_decoupling_pct,
        pa_hr_decoupling_pct,
        walk_seconds: r1(walk_sec),
        run_seconds: r1(run_sec),
        walk_segments: walk_segs,
        run_segments: run_segs,
        vt1_breach_seconds: r1(vt1_sec),
        vt2_breach_seconds: r1(vt2_sec),
        z5_breach_seconds: r1(z5_sec),
        directional_vectors,
        velocity_bands,
        laps: lap_rows,
        micro_events,
        temp_source: temp_source.to_string(),
    }
}

impl ForensicTelemetryReport {
    pub fn to_markdown_summary(&self) -> String {
        let mut lines = vec![
            format!(
                "- **1Hz Samples**: {} | **Walk vs Run**: {:.0}s ({} seg) / {:.0}s ({} seg)",
                self.sample_count, self.walk_seconds, self.walk_segments, self.run_seconds, self.run_segments
            ),
            format!("- **Temperature Source**: {}", self.temp_source),
        ];
        if let Some(pw) = self.pw_hr_decoupling_pct {
            lines.push(format!("- **Pw:Hr Decoupling (Active Run)**: {pw:.2}%"));
        }
        if let Some(pa) = self.pa_hr_decoupling_pct {
            lines.push(format!("- **Pa:Hr Decoupling (Active Run)**: {pa:.2}%"));
        }
        if let Some(over) = self.left_stance_over_52_8_pct {
            lines.push(format!("- **Left Stance > 52.8% L**: {over:.1}%"));
        }
        if let Some(ref resp) = self.respiration_brpm {
            lines.push(format!("- **Respiration**: mean {:.1} brpm (max {:.1})", resp.mean, resp.max));
        }
        if let Some(ref dir) = self.directional_vectors {
            lines.push(format!(
                "- **Outbound vs Inbound**: Out HR {:.0} / In HR {:.0} bpm (turnaround {:.0}m)",
                dir.outbound.mean_hr.unwrap_or(0.0),
                dir.inbound.mean_hr.unwrap_or(0.0),
                dir.turnaround_distance_m
            ));
        }
        lines.join("\n")
    }
}
