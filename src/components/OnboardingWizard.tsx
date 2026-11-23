import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Eye, EyeOff } from "lucide-react";
import {
  RACE_DISTANCE_PRESETS,
  presetDefaultName,
  presetDistanceKm,
  presetFromDistanceKm,
  type AthleteConfig,
  type AthleteType,
  type FirstUseFlowState,
  type GoalType,
  type ModelSettings,
  type RaceDistancePreset,
  type ThinkingLevel,
  type UserProfile,
} from "../types";

const HYROX_STATIONS = [
  "SkiErg",
  "Sled Push",
  "Sled Pull",
  "Burpee Broad Jumps",
  "Row",
  "Farmers Carry",
  "Sandbag Lunges",
  "Wall Balls",
];

function officialWeightsForDivision(division: string): string {
  const d = division.toLowerCase();
  if (d.includes("women") && d.includes("pro")) {
    return "Pro Women: Sled Push 152 kg · Sled Pull 103 kg · Farmers Carry 2×24 kg · Sandbag 20 kg · Wall Balls 6 kg";
  }
  if (d.includes("pro")) {
    return "Pro Men: Sled Push 202 kg · Sled Pull 153 kg · Farmers Carry 2×32 kg · Sandbag 30 kg · Wall Balls 9 kg";
  }
  if (d.includes("women")) {
    return "Open Women: Sled Push 102 kg · Sled Pull 78 kg · Farmers Carry 2×16 kg · Sandbag 10 kg · Wall Balls 4 kg";
  }
  return "Open Men / Doubles / Relay: Sled Push 152 kg · Sled Pull 103 kg · Farmers Carry 2×24 kg · Sandbag 20 kg · Wall Balls 6 kg";
}

function parseMmSs(raw: string): number | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  const parts = trimmed.split(":");
  if (parts.length !== 2) return null;
  const m = Number(parts[0]);
  const s = Number(parts[1]);
  if (!Number.isFinite(m) || !Number.isFinite(s) || m < 0 || s < 0 || s >= 60) return null;
  return Math.round(m * 60 + s);
}

function formatMmSs(sec: number | null | undefined): string {
  if (!sec || sec <= 0) return "";
  return `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, "0")}`;
}

export function OnboardingWizard({
  initialConfig,
  initialProfile,
  initialModelSettings,
  initialFirstUseState,
  hasGarminKey,
  hasGeminiKey,
  canDismiss,
  onDismiss,
  onComplete,
}: {
  initialConfig: AthleteConfig;
  initialProfile: UserProfile;
  initialModelSettings?: ModelSettings;
  initialFirstUseState?: FirstUseFlowState;
  hasGarminKey: boolean;
  hasGeminiKey: boolean;
  canDismiss: boolean;
  onDismiss: () => void;
  onComplete: () => Promise<void>;
}) {
  const [step, setStep] = useState<1 | 2 | 3 | 4>(1);
  const [busy, setBusy] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Step 1: Discipline & Biometrics
  const [athleteType, setAthleteType] = useState<AthleteType>(initialProfile.athlete_type ?? "runner");
  const [displayName, setDisplayName] = useState(initialConfig.specs.displayName ?? "");
  const [weightKg, setWeightKg] = useState(initialConfig.specs.weightKg?.toString() ?? "");
  const [heightCm, setHeightCm] = useState("175");
  const [sex, setSex] = useState("Unspecified");

  // Step 2: Garmin BYOK, Auto-Calibrate & Optional Intake Files (.pdf / .fit)
  const [garminKey, setGarminKey] = useState("");
  const [showGarminKey, setShowGarminKey] = useState(false);
  const [garminVerified, setGarminVerified] = useState(hasGarminKey);
  const [garminSkipped, setGarminSkipped] = useState(false);
  const [garminCheckState, setGarminCheckState] = useState<"idle" | "checking" | "verified" | "error">(
    hasGarminKey ? "verified" : "idle"
  );
  const [garminStatus, setGarminStatus] = useState<string | null>(
    hasGarminKey ? "Garmin Personal API Key already stored in Windows Credential Manager." : null
  );
  const [autoCalibrate, setAutoCalibrate] = useState(
    initialFirstUseState?.auto_calibrate_after_first_sync ?? true
  );
  const [queuedFiles, setQueuedFiles] = useState(
    initialFirstUseState?.queued_intake_files ?? []
  );
  const [maxHR, setMaxHR] = useState(initialConfig.specs.maxHR?.toString() ?? "");
  const [rhrBaseline, setRhrBaseline] = useState(initialConfig.specs.restingHRBaseline?.toString() ?? "");
  const [lthrBpm, setLthrBpm] = useState(initialProfile.runner_config.lthr_bpm?.toString() ?? "");

  // Step 3: Discipline Targets (Goal Time optional; HR-First & Pace Floor live in Settings -> Advanced & Coach intake)
  const initialGoalType: GoalType = initialConfig.objective.goalType ?? "general_fitness";
  const [goalType, setGoalType] = useState<GoalType>(initialGoalType);
  const [distancePreset, setDistancePreset] = useState<RaceDistancePreset>(() =>
    presetFromDistanceKm(initialConfig.objective.raceDistanceKm, initialGoalType !== "race")
  );
  const [raceDistanceKm, setRaceDistanceKm] = useState<string>(() =>
    initialConfig.objective.raceDistanceKm && initialConfig.objective.raceDistanceKm > 0
      ? initialConfig.objective.raceDistanceKm.toFixed(1)
      : ""
  );
  const [primaryDistance, setPrimaryDistance] = useState(
    initialConfig.objective.raceName ?? initialProfile.runner_config.primary_distance ?? ""
  );
  const [raceDate, setRaceDate] = useState(initialConfig.objective.raceDate ?? "");
  const [goalTime, setGoalTime] = useState(initialConfig.objective.targetTimeDescription ?? "");
  const [injuryNotes, setInjuryNotes] = useState(initialConfig.medical.conditionsAndNotes ?? "");

  const [division, setDivision] = useState(initialProfile.hyrox_config.division ?? "Men Open");
  const [watchLoggingMode, setWatchLoggingMode] = useState(
    initialProfile.hyrox_config.watch_logging_mode ?? "Manual Laps"
  );
  const [weakStations, setWeakStations] = useState<string[]>(
    initialProfile.hyrox_config.weak_stations ?? []
  );
  const [fresh1kMmSs, setFresh1kMmSs] = useState(
    formatMmSs(initialProfile.hyrox_config.fresh_1km_pace_sec) || "4:15"
  );
  const [comp1kMmSs, setComp1kMmSs] = useState(
    formatMmSs(initialProfile.hyrox_config.compromised_1km_target_sec) || "4:35"
  );

  // Step 4: Gemini BYOK & Global Model / Thinking Defaults
  const [geminiKey, setGeminiKey] = useState("");
  const [showGeminiKey, setShowGeminiKey] = useState(false);
  const [geminiVerified, setGeminiVerified] = useState(hasGeminiKey);
  const [geminiSkipped, setGeminiSkipped] = useState(false);
  const [geminiCheckState, setGeminiCheckState] = useState<"idle" | "checking" | "verified" | "error">(
    hasGeminiKey ? "verified" : "idle"
  );
  const [geminiStatus, setGeminiStatus] = useState<string | null>(
    hasGeminiKey ? "Gemini API key already stored in Windows Credential Manager." : null
  );
  const [geminiModel, setGeminiModel] = useState(
    initialModelSettings?.global_model_id ?? "gemini-4-hydrogen"
  );
  const [thinkingLevel, setThinkingLevel] = useState<ThinkingLevel>(
    initialModelSettings?.global_thinking_level ?? "MEDIUM"
  );

  function toggleWeakStation(station: string) {
    setWeakStations((prev) => {
      if (prev.includes(station)) return prev.filter((s) => s !== station);
      if (prev.length >= 2) return [prev[1], station];
      return [...prev, station];
    });
  }

  async function handleVerifyGarmin() {
    if (!garminKey.trim()) return;
    setBusy(true);
    setErrorMsg(null);
    setGarminCheckState("checking");
    setGarminStatus("Validating Garmin Personal API key…");
    try {
      await invoke("verify_garmin_key", { key: garminKey.trim() });
      setGarminVerified(true);
      setGarminSkipped(false);
      setGarminCheckState("verified");
      setGarminStatus("Connected (HTTP 200) — saved & verified in Windows Credential Manager.");
      setGarminKey("");
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setGarminVerified(false);
      setGarminCheckState("error");
      setGarminStatus(msg);
    } finally {
      setBusy(false);
    }
  }

  async function handleVerifyGemini() {
    if (!geminiKey.trim()) return;
    setBusy(true);
    setErrorMsg(null);
    setGeminiCheckState("checking");
    setGeminiStatus("Validating Gemini API key…");
    try {
      const res = await invoke<{ ok: boolean; http_status: number; message?: string }>("verify_gemini_key", {
        key: geminiKey.trim(),
        persist: true,
      });
      setGeminiVerified(true);
      setGeminiSkipped(false);
      setGeminiCheckState("verified");
      setGeminiStatus(
        res.message
          ? `Connected (HTTP ${res.http_status}) — ${res.message}`
          : "Connected (HTTP 200) — saved & verified in Windows Credential Manager."
      );
      setGeminiKey("");
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setGeminiVerified(false);
      setGeminiCheckState("error");
      setGeminiStatus(msg);
    } finally {
      setBusy(false);
    }
  }

  async function handlePickIntakeFiles() {
    const selected = await open({
      multiple: true,
      filters: [{ name: "Lab PDF or Garmin FIT", extensions: ["pdf", "fit"] }],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    if (paths.length === 0) return;
    setBusy(true);
    setErrorMsg(null);
    try {
      let latestState: FirstUseFlowState | null = null;
      for (const p of paths) {
        if (typeof p === "string" && p.trim()) {
          latestState = await invoke<FirstUseFlowState>("queue_intake_file", { sourcePath: p });
        }
      }
      if (latestState) {
        setQueuedFiles(latestState.queued_intake_files);
      }
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleRemoveIntakeFile(fileId: string) {
    setBusy(true);
    setErrorMsg(null);
    try {
      const updated = await invoke<FirstUseFlowState>("remove_intake_file", { fileId });
      setQueuedFiles(updated.queued_intake_files);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleFinish(skipGemini = false) {
    setBusy(true);
    setErrorMsg(null);
    try {
      if (!skipGemini && geminiKey.trim() && !geminiVerified) {
        await invoke("verify_gemini_key", { key: geminiKey.trim(), persist: true });
      }
      await invoke("save_global_model_settings", {
        modelId: geminiModel,
        thinkingLevel,
      });
      await invoke("set_auto_calibrate_after_sync", { enabled: autoCalibrate });

      const parsedWeightKg = weightKg.trim() ? Number(weightKg) : null;
      const validWeightKg = parsedWeightKg && Number.isFinite(parsedWeightKg) && parsedWeightKg > 0 ? parsedWeightKg : null;
      const cleanGoalTime = goalTime.trim();
      const parsedDistRaw = raceDistanceKm.trim() ? Number(raceDistanceKm.replace(",", ".")) : null;
      const resolvedDistKm =
        parsedDistRaw && Number.isFinite(parsedDistRaw) && parsedDistRaw > 0
          ? parsedDistRaw
          : presetDistanceKm(distancePreset);
      const trimmedRaceName = primaryDistance.trim();
      const resolvedPrimaryDist =
        trimmedRaceName ||
        presetDefaultName(distancePreset) ||
        (resolvedDistKm ? `${resolvedDistKm.toFixed(1)} km` : null);
      const resolvedGoalType: GoalType = athleteType === "hyrox" ? "hybrid_hyrox" : goalType;
      const nextConfig: AthleteConfig = {
        ...initialConfig,
        specs: {
          ...initialConfig.specs,
          displayName: displayName.trim(),
          weightKg: validWeightKg,
          weightBaselineConfirmed: Boolean(validWeightKg),
          maxHR: maxHR ? Number(maxHR) : null,
          restingHRBaseline: rhrBaseline ? Number(rhrBaseline) : null,
        },
        medical: {
          ...initialConfig.medical,
          conditionsAndNotes: injuryNotes.trim(),
        },
        objective: {
          ...initialConfig.objective,
          goalType: resolvedGoalType,
          raceName: athleteType === "hyrox" ? `HYROX (${division})` : trimmedRaceName || null,
          raceDistanceKm: resolvedDistKm,
          raceDate: raceDate.trim() || null,
          targetTimeDescription: cleanGoalTime ? cleanGoalTime : null,
          hybridProfile:
            athleteType === "hyrox" || athleteType === "hybrid_both"
              ? {
                  division,
                  focusStations: weakStations,
                  weeklyHybridSessionsTarget: 3,
                }
              : null,
        },
        discipline:
          athleteType === "hyrox"
            ? "hyrox"
            : athleteType === "hybrid_both"
            ? "hybrid"
            : "running",
        updatedAt: new Date().toISOString(),
      };
      await invoke("save_athlete_config", { config: nextConfig });

      const nextProfile: UserProfile = {
        athlete_type: athleteType,
        onboarding_completed: true,
        runner_config: {
          ...initialProfile.runner_config,
          primary_distance: resolvedPrimaryDist,
          hr_first_governor: initialProfile.runner_config.hr_first_governor ?? true,
          pace_floor_sec_per_km: initialProfile.runner_config.pace_floor_sec_per_km ?? null,
          z2_range_bpm: initialProfile.runner_config.z2_range_bpm,
          z3_range_bpm: initialProfile.runner_config.z3_range_bpm,
          lthr_bpm: lthrBpm ? Number(lthrBpm) : null,
          max_hr_bpm: maxHR ? Number(maxHR) : initialProfile.runner_config.max_hr_bpm ?? null,
          resting_hr_bpm: rhrBaseline ? Number(rhrBaseline) : initialProfile.runner_config.resting_hr_bpm ?? null,
        },
        hyrox_config: {
          division: athleteType === "runner" ? null : division,
          watch_logging_mode: athleteType === "runner" ? null : watchLoggingMode,
          target_total_time_sec: initialProfile.hyrox_config.target_total_time_sec,
          fresh_1km_pace_sec: parseMmSs(fresh1kMmSs),
          compromised_1km_target_sec: parseMmSs(comp1kMmSs),
          weak_stations: weakStations,
        },
      };
      await invoke("save_user_profile", { profile: nextProfile });

      // Ensure guided first-use intake starts cleanly without dumping starter templates or auto-firing PLANNING calls
      if (!initialFirstUseState || initialFirstUseState.stage === "intake") {
        await invoke("advance_first_use_stage", { stage: "intake" });
      }

      await onComplete();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 50,
        background: "rgba(9, 13, 20, 0.85)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: 24,
      }}
    >
      <div className="card" style={{ width: "min(760px, 100%)", maxHeight: "90vh", overflowY: "auto" }}>
        <div className="card-header">
          <div>
            <strong>Splits Coach Onboarding · Step {step} of 4</strong>
            <div className="muted" style={{ fontSize: 12 }}>
              BYOK Private Setup · Garmin Personal API + Local SQLite + Gemini
            </div>
          </div>
          {canDismiss && (
            <button className="secondary" onClick={onDismiss}>
              Close
            </button>
          )}
        </div>

        {errorMsg && (
          <div className="pill red" style={{ marginBottom: 12, display: "block" }}>
            {errorMsg}
          </div>
        )}

        {step === 1 && (
          <div>
            <h3>Step 1 — Discipline & Biometrics</h3>
            <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 10, marginBottom: 14 }}>
              {(
                [
                  { id: "runner", title: "Running", desc: "Road, trail & marathon aerobic/VT1 progression." },
                  { id: "hyrox", title: "HYROX", desc: "8×1km compromised running + official station loads." },
                  { id: "hybrid_both", title: "Hybrid (Mixed)", desc: "Concurrent running & HYROX/strength blocks." },
                ] as const
              ).map((opt) => (
                <button
                  key={opt.id}
                  type="button"
                  className={athleteType === opt.id ? "primary" : "secondary"}
                  style={{ textAlign: "left", padding: 12 }}
                  onClick={() => {
                    const prev = athleteType;
                    setAthleteType(opt.id);
                    if (opt.id === "hyrox") {
                      setGoalType("hybrid_hyrox");
                    } else if (prev === "hyrox" && goalType === "hybrid_hyrox") {
                      setGoalType("general_fitness");
                      setDistancePreset("none");
                      setRaceDistanceKm("");
                    }
                  }}
                >
                  <div style={{ fontWeight: 600 }}>{opt.title}</div>
                  <div style={{ fontSize: 12, opacity: 0.85, marginTop: 4 }}>{opt.desc}</div>
                </button>
              ))}
            </div>
            <div className="form-row">
              <div>
                <label className="muted">Display Name</label>
                <input value={displayName} onChange={(e) => setDisplayName(e.target.value)} placeholder="Your name" />
              </div>
              <div>
                <label className="muted">Weight (kg)</label>
                <input type="number" value={weightKg} onChange={(e) => setWeightKg(e.target.value)} placeholder="70" />
              </div>
              <div>
                <label className="muted">Height (cm)</label>
                <input type="number" value={heightCm} onChange={(e) => setHeightCm(e.target.value)} placeholder="175" />
              </div>
              <div>
                <label className="muted">Sex</label>
                <select value={sex} onChange={(e) => setSex(e.target.value)}>
                  <option value="Unspecified">Prefer not to say</option>
                  <option value="Male">Male</option>
                  <option value="Female">Female</option>
                </select>
              </div>
            </div>
          </div>
        )}

        {step === 2 && (
          <div>
            <h3>Step 2 — Telemetry &amp; Calibration (BYOK)</h3>
            <p className="muted" style={{ fontSize: 13 }}>
              In Garmin Connect Web (<strong>Settings → Personal API Key</strong>), generate a 30/90/180/365-day key.
              Garmin displays the key only once upon creation — paste it below to validate and store securely in Windows Credential Manager.
            </p>
            <div className="key-input-group" style={{ marginBottom: 8 }}>
              <input
                type={showGarminKey ? "text" : "password"}
                placeholder={garminVerified ? "Garmin key verified (paste new key to replace)" : "Garmin Personal API Key"}
                value={garminKey}
                onChange={(e) => {
                  setGarminKey(e.target.value);
                  setGarminVerified(false);
                  setGarminSkipped(false);
                  setGarminCheckState("idle");
                  setGarminStatus(null);
                }}
              />
              <button
                type="button"
                className="secondary"
                aria-label={showGarminKey ? "Hide Garmin API key" : "Show Garmin API key"}
                onClick={() => setShowGarminKey((v) => !v)}
              >
                {showGarminKey ? <EyeOff size={15} /> : <Eye size={15} />}
              </button>
              <button
                type="button"
                className="primary"
                disabled={busy || !garminKey.trim()}
                onClick={() => void handleVerifyGarmin()}
              >
                {garminCheckState === "checking" ? "Validating…" : "Verify & Save Key"}
              </button>
              <button
                type="button"
                className="secondary"
                disabled={busy}
                onClick={() => {
                  setGarminSkipped(true);
                  setGarminCheckState("idle");
                  setGarminStatus("Skipped Garmin setup for now — you can connect later in Settings.");
                  setStep(3);
                }}
              >
                Skip Garmin for now
              </button>
            </div>
            {garminStatus && (
              <div
                className={
                  garminCheckState === "verified"
                    ? "key-status-ok"
                    : garminCheckState === "error"
                    ? "key-status-err"
                    : "muted"
                }
                style={{ marginBottom: 14 }}
              >
                {garminVerified ? "✓ " : ""}{garminStatus}
              </div>
            )}

            <div style={{ borderTop: "1px solid var(--border)", paddingTop: 12, marginTop: 8 }}>
              <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13, fontWeight: 600, marginBottom: 10 }}>
                <input
                  type="checkbox"
                  style={{ width: "auto" }}
                  checked={autoCalibrate}
                  onChange={(e) => setAutoCalibrate(e.target.checked)}
                />
                Auto-calibrate HR &amp; pace thresholds after first Garmin sync
              </label>

              <div className="dropzone" style={{ marginBottom: 12 }}>
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 10 }}>
                  <div>
                    <strong style={{ fontSize: 13 }}>Optional Intake File: Lab Stress Test (.pdf) or Garmin Stress/Lactate Test (.fit)</strong>
                    <div className="muted" style={{ fontSize: 12 }}>
                      Queued privately for Coach intake — nothing is sent until you start onboarding in Coach.
                    </div>
                  </div>
                  <button type="button" className="secondary" disabled={busy} onClick={() => void handlePickIntakeFiles()}>
                    Upload .pdf / .fit
                  </button>
                </div>
                {queuedFiles.length > 0 && (
                  <div style={{ display: "flex", flexDirection: "column", gap: 6, marginTop: 10 }}>
                    {queuedFiles.map((f) => (
                      <div
                        key={f.id}
                        style={{
                          display: "flex",
                          justifyContent: "space-between",
                          alignItems: "center",
                          padding: "6px 8px",
                          borderRadius: 6,
                          background: "var(--card)",
                          border: "1px solid var(--border)",
                          fontSize: 12,
                        }}
                      >
                        <span>
                          <strong>{f.file_name}</strong>{" "}
                          <span className="muted">({f.kind})</span>
                        </span>
                        <button
                          type="button"
                          className="secondary"
                          style={{ padding: "3px 8px", fontSize: 11 }}
                          disabled={busy}
                          onClick={() => void handleRemoveIntakeFile(f.id)}
                        >
                          Remove
                        </button>
                      </div>
                    ))}
                  </div>
                )}
              </div>

              <div className="form-row">
                <div>
                  <label className="muted">Max HR (bpm, optional)</label>
                  <input type="number" value={maxHR} onChange={(e) => setMaxHR(e.target.value)} placeholder="Auto-calibrated if blank" />
                </div>
                <div>
                  <label className="muted">Resting HR Baseline (bpm, optional)</label>
                  <input type="number" value={rhrBaseline} onChange={(e) => setRhrBaseline(e.target.value)} placeholder="Auto-calibrated if blank" />
                </div>
                <div>
                  <label className="muted">Estimated LTHR (bpm, optional)</label>
                  <input type="number" value={lthrBpm} onChange={(e) => setLthrBpm(e.target.value)} placeholder="Auto-calibrated if blank" />
                </div>
              </div>
            </div>
          </div>
        )}

        {step === 3 && (
          <div>
            <h3>Step 3 — Targets &amp; Context</h3>
            {(athleteType === "runner" || athleteType === "hybrid_both") && (
              <div style={{ marginBottom: 14 }}>
                <strong>Endurance Running Targets</strong>
                <div className="form-row" style={{ marginTop: 8 }}>
                  <div>
                    <label className="muted">Goal Type</label>
                    <select
                      value={goalType}
                      onChange={(e) => {
                        const prevGoal = goalType;
                        const nextGoal = e.target.value as GoalType;
                        setGoalType(nextGoal);
                        if (nextGoal === "couch_to_5k") {
                          setDistancePreset("5k");
                          setRaceDistanceKm("5.0");
                        } else if (nextGoal === "race") {
                          if (distancePreset === "none") {
                            setDistancePreset("5k");
                            setRaceDistanceKm("5.0");
                          }
                        } else if (prevGoal === "couch_to_5k" || prevGoal === "race") {
                          setDistancePreset("none");
                          setRaceDistanceKm("");
                        }
                      }}
                    >
                      <option value="general_fitness">General Fitness</option>
                      <option value="base_building">Base Building</option>
                      <option value="race">Target Race</option>
                      <option value="return_from_injury">Return from Injury</option>
                      <option value="return_to_running">Back to Running (Re-Entry)</option>
                      <option value="couch_to_5k">Couch to 5K (C25K)</option>
                      {athleteType === "hybrid_both" && (
                        <option value="hybrid_hyrox">Hyrox / Hybrid Athlete</option>
                      )}
                    </select>
                  </div>
                  <div>
                    <label className="muted">Target Distance</label>
                    <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                      <select
                        value={distancePreset}
                        onChange={(e) => {
                          const nextPreset = e.target.value as RaceDistancePreset;
                          setDistancePreset(nextPreset);
                          const km = presetDistanceKm(nextPreset);
                          if (km !== null) {
                            setRaceDistanceKm(km.toFixed(1));
                          } else if (nextPreset === "none") {
                            setRaceDistanceKm("");
                          }
                        }}
                      >
                        {RACE_DISTANCE_PRESETS.filter(
                          (p) => p.id !== "none" || goalType !== "race"
                        ).map((p) => (
                          <option key={p.id} value={p.id}>
                            {p.label}
                          </option>
                        ))}
                      </select>
                      {distancePreset !== "none" && (
                        <div style={{ display: "flex", alignItems: "center", gap: 4, flexShrink: 0 }}>
                          <input
                            type="number"
                            step="0.1"
                            min="0"
                            style={{ width: 78 }}
                            placeholder="km"
                            value={raceDistanceKm}
                            onChange={(e) => {
                              const raw = e.target.value;
                              setRaceDistanceKm(raw);
                              const norm = raw.replace(",", ".").trim();
                              if (!norm) {
                                if (distancePreset !== "custom" && goalType !== "race") {
                                  setDistancePreset("none");
                                }
                              } else {
                                const parsed = Number(norm);
                                if (Number.isFinite(parsed) && parsed > 0) {
                                  setDistancePreset(presetFromDistanceKm(parsed, goalType !== "race"));
                                }
                              }
                            }}
                          />
                          <span className="muted" style={{ fontSize: 12 }}>km</span>
                        </div>
                      )}
                    </div>
                  </div>
                  <div>
                    <label className="muted">Race / Event Name (optional)</label>
                    <input
                      placeholder="Optional (e.g., City 5K)"
                      value={primaryDistance}
                      onChange={(e) => setPrimaryDistance(e.target.value)}
                    />
                  </div>
                  <div>
                    <label className="muted">Target Race Date</label>
                    <input type="date" value={raceDate} onChange={(e) => setRaceDate(e.target.value)} />
                  </div>
                  <div style={{ gridColumn: "span 2" }}>
                    <label className="muted">Target Finish Time (optional)</label>
                    <input
                      placeholder="HH:MM:SS — leave blank if goal is to finish"
                      value={goalTime}
                      onChange={(e) => setGoalTime(e.target.value)}
                    />
                  </div>
                </div>
                {goalType === "couch_to_5k" && (
                  <div style={{ color: "var(--accent-primary)", fontSize: 12, marginTop: 4 }}>
                    C25K targets a progressive 8-9 week buildup from run-walk intervals to 5.0 km continuous running.
                  </div>
                )}
                <div className="muted" style={{ fontSize: 12, marginTop: 4 }}>
                  Advanced guardrails (HR-First Governor and Hard Pace Floor) can be discussed conversationally with your Coach or configured anytime in <strong>Settings → Advanced Guardrails</strong>.
                </div>
                <div style={{ marginTop: 10 }}>
                  <label className="muted">Injury / Structural Notes</label>
                  <input
                    placeholder="e.g. Left Achilles history, keep 36h between hard sessions"
                    value={injuryNotes}
                    onChange={(e) => setInjuryNotes(e.target.value)}
                  />
                </div>
              </div>
            )}

            {(athleteType === "hyrox" || athleteType === "hybrid_both") && (
              <div style={{ borderTop: athleteType === "hybrid_both" ? "1px solid var(--border)" : "none", paddingTop: 10 }}>
                <strong>HYROX Division & Station Targets</strong>
                <div className="form-row" style={{ marginTop: 8 }}>
                  <div>
                    <label className="muted">Division</label>
                    <select value={division} onChange={(e) => setDivision(e.target.value)}>
                      <option value="Men Open">Men Open</option>
                      <option value="Men Pro">Men Pro</option>
                      <option value="Women Open">Women Open</option>
                      <option value="Women Pro">Women Pro</option>
                      <option value="Doubles">Doubles</option>
                      <option value="Relay">Relay</option>
                    </select>
                  </div>
                  <div>
                    <label className="muted">Watch Logging Mode</label>
                    <select value={watchLoggingMode} onChange={(e) => setWatchLoggingMode(e.target.value)}>
                      <option value="Manual Laps">Manual Laps</option>
                      <option value="Multisport/Cardio">Multisport / Cardio</option>
                      <option value="Connect IQ ROXFIT/RoxTrack">Connect IQ ROXFIT / RoxTrack</option>
                    </select>
                  </div>
                  <div>
                    <label className="muted">Fresh 1km Pace (mm:ss)</label>
                    <input value={fresh1kMmSs} onChange={(e) => setFresh1kMmSs(e.target.value)} placeholder="4:15" />
                  </div>
                  <div>
                    <label className="muted">Target Compromised 1km (mm:ss)</label>
                    <input value={comp1kMmSs} onChange={(e) => setComp1kMmSs(e.target.value)} placeholder="4:35" />
                  </div>
                </div>
                <div className="muted" style={{ fontSize: 12, marginTop: 8 }}>
                  Official Weights: {officialWeightsForDivision(division)}
                </div>
                <div style={{ marginTop: 10 }}>
                  <label className="muted">Weakest 2 Stations (select up to 2)</label>
                  <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginTop: 6 }}>
                    {HYROX_STATIONS.map((st) => {
                      const active = weakStations.includes(st);
                      return (
                        <button
                          key={st}
                          type="button"
                          className={active ? "primary" : "secondary"}
                          style={{ padding: "5px 10px", fontSize: 12 }}
                          onClick={() => toggleWeakStation(st)}
                        >
                          {st}
                        </button>
                      );
                    })}
                  </div>
                </div>
              </div>
            )}
          </div>
        )}

        {step === 4 && (
          <div>
            <h3>Step 4 — Gemini AI Key &amp; Global Model</h3>
            <p className="muted" style={{ fontSize: 13 }}>
              {hasGeminiKey
                ? "A Gemini API key is already saved. Paste a new key and validate it, or continue with your saved key."
                : "Paste your Gemini API key (stored in Windows Credential Manager) to validate and enable live streaming AI coaching."}
            </p>
            <div style={{ marginBottom: 12 }}>
              <label className="muted">Gemini API Key</label>
              <div className="key-input-group" style={{ marginTop: 4 }}>
                <input
                  type={showGeminiKey ? "text" : "password"}
                  placeholder={hasGeminiKey ? "Saved (leave blank to keep)" : "AIza..."}
                  value={geminiKey}
                  onChange={(e) => {
                    const val = e.target.value;
                    setGeminiKey(val);
                    if (!val.trim() && hasGeminiKey) {
                      setGeminiVerified(true);
                      setGeminiCheckState("verified");
                      setGeminiStatus("Using existing Gemini API key in Windows Credential Manager.");
                    } else {
                      setGeminiVerified(false);
                      setGeminiSkipped(false);
                      setGeminiCheckState("idle");
                      setGeminiStatus(null);
                    }
                  }}
                />
                <button
                  type="button"
                  className="secondary"
                  aria-label={showGeminiKey ? "Hide Gemini API key" : "Show Gemini API key"}
                  onClick={() => setShowGeminiKey((v) => !v)}
                >
                  {showGeminiKey ? <EyeOff size={15} /> : <Eye size={15} />}
                </button>
                <button
                  type="button"
                  className="primary"
                  disabled={busy || !geminiKey.trim()}
                  onClick={() => void handleVerifyGemini()}
                >
                  {geminiCheckState === "checking" ? "Validating…" : "Validate & Save Key"}
                </button>
                <button
                  type="button"
                  className="secondary"
                  disabled={busy}
                  onClick={() => {
                    setGeminiSkipped(true);
                    setGeminiCheckState("idle");
                    void handleFinish(true);
                  }}
                >
                  Skip Gemini for now
                </button>
              </div>
              {geminiStatus && (
                <div
                  className={
                    geminiCheckState === "verified"
                      ? "key-status-ok"
                      : geminiCheckState === "error"
                      ? "key-status-err"
                      : "muted"
                  }
                  style={{ marginTop: 6 }}
                >
                  {geminiVerified ? "✓ " : ""}{geminiStatus}
                </div>
              )}
            </div>
            <div className="form-row">
              <div>
                <label className="muted">Global Default Model</label>
                <select value={geminiModel} onChange={(e) => setGeminiModel(e.target.value)}>
                  <option value="gemini-4-hydrogen">Hydrogen (gemini-4-hydrogen)</option>
                  <option value="gemini-3.1-pro-preview">Gemini 3.1 Pro</option>
                  <option value="gemini-2.5-pro">Gemini 2.5 Pro</option>
                  <option value="gemini-2.5-flash">Gemini 2.5 Flash</option>
                </select>
              </div>
              <div>
                <label className="muted">Global Thinking Level</label>
                <select
                  value={thinkingLevel}
                  onChange={(e) => setThinkingLevel(e.target.value as ThinkingLevel)}
                >
                  <option value="LOW">Low</option>
                  <option value="MEDIUM">Medium (Recommended)</option>
                  <option value="HIGH">High</option>
                </select>
              </div>
            </div>
            <p className="muted" style={{ fontSize: 12 }}>
              You will enter the guided Coach intake next — nothing is auto-synced or generated without your confirmation.
            </p>
          </div>
        )}

        <div style={{ display: "flex", justifyContent: "space-between", marginTop: 20 }}>
          <button
            className="secondary"
            disabled={step === 1 || busy}
            onClick={() => setStep((s) => (s > 1 ? ((s - 1) as 1 | 2 | 3 | 4) : s))}
          >
            Back
          </button>
          {step < 4 ? (
            <button
              className="primary"
              disabled={busy || (step === 2 && !(garminVerified || garminSkipped))}
              onClick={() => setStep((s) => (s < 4 ? ((s + 1) as 1 | 2 | 3 | 4) : s))}
            >
              Next
            </button>
          ) : (
            <button
              className="primary"
              disabled={busy || !(geminiVerified || geminiSkipped)}
              onClick={() => void handleFinish(false)}
            >
              {busy ? "Saving…" : "Complete Setup"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
