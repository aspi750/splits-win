import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { AthleteConfig, AthleteType, FitBaselineProbe, UserProfile } from "../types";

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
  hasGarminKey,
  hasGeminiKey,
  canDismiss,
  onDismiss,
  onComplete,
}: {
  initialConfig: AthleteConfig;
  initialProfile: UserProfile;
  hasGarminKey: boolean;
  hasGeminiKey: boolean;
  canDismiss: boolean;
  onDismiss: () => void;
  onComplete: (initialCoachPrompt: string) => Promise<void>;
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

  // Step 2: Garmin BYOK & FIT Probe
  const [garminKey, setGarminKey] = useState("");
  const [garminVerified, setGarminVerified] = useState(hasGarminKey);
  const [garminStatus, setGarminStatus] = useState<string | null>(
    hasGarminKey ? "Garmin Personal API Key already stored in Credential Manager." : null
  );
  const [probe, setProbe] = useState<FitBaselineProbe | null>(null);
  const [maxHR, setMaxHR] = useState(initialConfig.specs.maxHR?.toString() ?? "");
  const [rhrBaseline, setRhrBaseline] = useState(initialConfig.specs.restingHRBaseline?.toString() ?? "");
  const [lthrBpm, setLthrBpm] = useState(initialProfile.runner_config.lthr_bpm?.toString() ?? "");

  // Step 3: Discipline Targets & Guardrails
  const [primaryDistance, setPrimaryDistance] = useState(
    initialProfile.runner_config.primary_distance ?? "Half Marathon"
  );
  const [raceDate, setRaceDate] = useState(initialConfig.objective.raceDate ?? "");
  const [goalTime, setGoalTime] = useState(initialConfig.objective.targetTimeDescription ?? "");
  const [hrFirst, setHrFirst] = useState(initialProfile.runner_config.hr_first_governor ?? true);
  const [paceFloorMmSs, setPaceFloorMmSs] = useState(
    formatMmSs(initialProfile.runner_config.pace_floor_sec_per_km ?? initialConfig.medical.minPaceSecPerKm)
  );
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

  // Step 4: Gemini BYOK & Model
  const [geminiKey, setGeminiKey] = useState("");
  const [geminiModel, setGeminiModel] = useState("gemini-4-hydrogen");

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
    try {
      await invoke("verify_garmin_key", { key: garminKey.trim() });
      setGarminVerified(true);
      setGarminStatus("Verified & cached 24h bearer token in Windows Credential Manager.");
      setGarminKey("");
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleSaveGarminOnly() {
    if (!garminKey.trim()) return;
    setBusy(true);
    setErrorMsg(null);
    try {
      await invoke("save_garmin_key", { key: garminKey.trim() });
      setGarminVerified(true);
      setGarminStatus("Saved to Windows Credential Manager.");
      setGarminKey("");
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleProbeFitFolder() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected !== "string") return;
    setBusy(true);
    setErrorMsg(null);
    try {
      const res = await invoke<FitBaselineProbe>("probe_fit_baseline", { folderPath: selected });
      setProbe(res);
      if (res.detected_max_hr && !maxHR) setMaxHR(String(res.detected_max_hr));
      if (res.detected_rhr_baseline && !rhrBaseline) setRhrBaseline(String(res.detected_rhr_baseline));
      if (res.estimated_lthr && !lthrBpm) setLthrBpm(String(res.estimated_lthr));
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleFinish() {
    setBusy(true);
    setErrorMsg(null);
    try {
      if (geminiKey.trim()) {
        await invoke("save_gemini_key", { key: geminiKey.trim() });
      }
      const paceFloorSec = parseMmSs(paceFloorMmSs);
      const nextConfig: AthleteConfig = {
        ...initialConfig,
        specs: {
          ...initialConfig.specs,
          displayName: displayName.trim(),
          weightKg: weightKg ? Number(weightKg) : null,
          maxHR: maxHR ? Number(maxHR) : null,
          restingHRBaseline: rhrBaseline ? Number(rhrBaseline) : null,
        },
        medical: {
          ...initialConfig.medical,
          conditionsAndNotes: injuryNotes.trim(),
          enforceEasyHRCeiling: hrFirst,
          enforcePaceFloor: paceFloorSec != null,
          minPaceSecPerKm: paceFloorSec,
        },
        objective: {
          ...initialConfig.objective,
          goalType: athleteType === "runner" ? "race" : "hybrid_hyrox",
          raceName: athleteType === "hyrox" ? `HYROX (${division})` : primaryDistance,
          raceDate: raceDate.trim() || null,
          targetTimeDescription: goalTime.trim() || null,
          hybridProfile:
            athleteType === "hyrox" || athleteType === "hybrid_both"
              ? {
                  division,
                  focusStations: weakStations,
                  weeklyHybridSessionsTarget: 3,
                }
              : null,
        },
        updatedAt: new Date().toISOString(),
      };
      await invoke("save_athlete_config", { config: nextConfig });

      const nextProfile: UserProfile = {
        athlete_type: athleteType,
        onboarding_completed: true,
        runner_config: {
          primary_distance: primaryDistance || null,
          hr_first_governor: hrFirst,
          pace_floor_sec_per_km: paceFloorSec,
          z2_range_bpm: initialProfile.runner_config.z2_range_bpm,
          z3_range_bpm: initialProfile.runner_config.z3_range_bpm,
          lthr_bpm: lthrBpm ? Number(lthrBpm) : null,
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

      const prompt =
        athleteType === "hyrox"
          ? `I just completed onboarding as a ${division} HYROX athlete (biometrics: ${weightKg || "–"} kg, ${heightCm} cm, ${sex}; weak stations: ${weakStations.join(", ") || "none selected"}; model: ${geminiModel}). Review my synced history and generate my initial HYROX baseline & starter COACH_PROFILE.md / TRAINING_PLAN.md.`
          : athleteType === "hybrid_both"
          ? `I just completed onboarding as a Hybrid Athlete (${primaryDistance} + HYROX ${division}; weak stations: ${weakStations.join(", ") || "none selected"}). Review my synced history and generate my initial hybrid baseline & starter COACH_PROFILE.md / TRAINING_PLAN.md.`
          : `I just completed onboarding as an Endurance Runner targeting ${primaryDistance} (HR-first: ${hrFirst ? "on" : "off"}, max HR: ${maxHR || "auto"}, LTHR: ${lthrBpm || "auto"}). Review my synced history and generate my initial baseline & starter COACH_PROFILE.md / TRAINING_PLAN.md.`;

      await onComplete(prompt);
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
                  { id: "runner", title: "Endurance Runner", desc: "Road, trail & marathon aerobic/VT1 progression." },
                  { id: "hyrox", title: "HYROX Athlete", desc: "8×1km compromised running + official station loads." },
                  { id: "hybrid_both", title: "Hybrid Athlete (Both)", desc: "Concurrent running & HYROX/strength blocks." },
                ] as const
              ).map((opt) => (
                <button
                  key={opt.id}
                  type="button"
                  className={athleteType === opt.id ? "primary" : "secondary"}
                  style={{ textAlign: "left", padding: 12 }}
                  onClick={() => setAthleteType(opt.id)}
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
            <h3>Step 2 — Garmin Telemetry & Auto-Calibration (BYOK)</h3>
            <p className="muted" style={{ fontSize: 13 }}>
              In Garmin Connect Web (<strong>Settings → Personal API Key</strong>), generate a 30/90/180/365-day key.
              Garmin displays the key only once upon creation — paste it below to store securely in Windows Credential Manager.
            </p>
            <div style={{ display: "flex", gap: 8, marginBottom: 10 }}>
              <input
                type="password"
                placeholder="Garmin Personal API Key"
                value={garminKey}
                onChange={(e) => setGarminKey(e.target.value)}
              />
              <button className="primary" disabled={busy || !garminKey.trim()} onClick={() => void handleVerifyGarmin()}>
                Verify & Save Key
              </button>
              <button className="secondary" disabled={busy || !garminKey.trim()} onClick={() => void handleSaveGarminOnly()}>
                Save Only
              </button>
            </div>
            {garminStatus && (
              <div className="muted" style={{ fontSize: 12, marginBottom: 14 }}>
                {garminVerified ? "✓ " : ""}{garminStatus}
              </div>
            )}

            <div style={{ borderTop: "1px solid var(--border)", paddingTop: 12, marginTop: 8 }}>
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
                <strong>Optional: Auto-Calibrate from Local .fit Folder</strong>
                <button className="secondary" disabled={busy} onClick={() => void handleProbeFitFolder()}>
                  Select .fit Folder & Probe
                </button>
              </div>
              {probe && (
                <div className="muted" style={{ fontSize: 12, marginBottom: 10 }}>
                  Scanned {probe.scanned_files} .fit file(s) · Est. weekly volume:{" "}
                  {probe.weekly_km_est != null ? `${probe.weekly_km_est} km/wk` : "–"}
                </div>
              )}
              <div className="form-row">
                <div>
                  <label className="muted">Max HR (bpm)</label>
                  <input type="number" value={maxHR} onChange={(e) => setMaxHR(e.target.value)} placeholder="190" />
                </div>
                <div>
                  <label className="muted">Resting HR Baseline (bpm)</label>
                  <input type="number" value={rhrBaseline} onChange={(e) => setRhrBaseline(e.target.value)} placeholder="50" />
                </div>
                <div>
                  <label className="muted">Estimated LTHR (bpm)</label>
                  <input type="number" value={lthrBpm} onChange={(e) => setLthrBpm(e.target.value)} placeholder="170" />
                </div>
              </div>
            </div>
          </div>
        )}

        {step === 3 && (
          <div>
            <h3>Step 3 — Discipline Targets & Guardrails</h3>
            {(athleteType === "runner" || athleteType === "hybrid_both") && (
              <div style={{ marginBottom: 14 }}>
                <strong>Endurance Running Targets</strong>
                <div className="form-row" style={{ marginTop: 8 }}>
                  <div>
                    <label className="muted">Primary Distance / Event</label>
                    <input value={primaryDistance} onChange={(e) => setPrimaryDistance(e.target.value)} />
                  </div>
                  <div>
                    <label className="muted">Target Race Date</label>
                    <input type="date" value={raceDate} onChange={(e) => setRaceDate(e.target.value)} />
                  </div>
                  <div>
                    <label className="muted">Goal Time</label>
                    <input placeholder="e.g. Sub 1:35" value={goalTime} onChange={(e) => setGoalTime(e.target.value)} />
                  </div>
                  <div>
                    <label className="muted">Hard Pace Floor (mm:ss/km)</label>
                    <input placeholder="e.g. 6:30" value={paceFloorMmSs} onChange={(e) => setPaceFloorMmSs(e.target.value)} />
                  </div>
                </div>
                <label style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 8, fontSize: 13 }}>
                  <input type="checkbox" checked={hrFirst} onChange={(e) => setHrFirst(e.target.checked)} />
                  HR-First Governor (enforce VT1 / easy HR ceiling)
                </label>
                <div style={{ marginTop: 8 }}>
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
            <h3>Step 4 — Gemini AI Key & Baseline Launch</h3>
            <p className="muted" style={{ fontSize: 13 }}>
              {hasGeminiKey
                ? "A Gemini API key is already saved. You can leave this blank or paste a new key to replace it."
                : "Paste your Gemini API key (stored in Windows Credential Manager) to enable live streaming AI coaching."}
            </p>
            <div className="form-row">
              <div>
                <label className="muted">Gemini API Key</label>
                <input
                  type="password"
                  placeholder={hasGeminiKey ? "Saved (leave blank to keep)" : "AIza..."}
                  value={geminiKey}
                  onChange={(e) => setGeminiKey(e.target.value)}
                />
              </div>
              <div>
                <label className="muted">Default Coach Model</label>
                <select value={geminiModel} onChange={(e) => setGeminiModel(e.target.value)}>
                  <option value="gemini-4-hydrogen">gemini-4-hydrogen (Recommended)</option>
                  <option value="gemini-2.5-pro">gemini-2.5-pro</option>
                </select>
              </div>
            </div>
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
              disabled={busy}
              onClick={() => setStep((s) => (s < 4 ? ((s + 1) as 1 | 2 | 3 | 4) : s))}
            >
              Next
            </button>
          ) : (
            <button className="primary" disabled={busy} onClick={() => void handleFinish()}>
              {busy ? "Saving…" : "Save & Generate Initial Athlete Baseline"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
