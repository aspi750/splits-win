import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Activity,
  Check,
  Clipboard,
  MessageSquare,
  RefreshCw,
  Settings,
  Sun,
} from "lucide-react";
import type { AthleteConfig, BootstrapState, ExecutionMode, GoalType } from "./types";

type NavTab = "today" | "activities" | "coach" | "settings";

interface ChatMessage {
  role: "user" | "assistant";
  text: string;
}

export default function App() {
  const [state, setState] = useState<BootstrapState | null>(null);
  const [tab, setTab] = useState<NavTab>("today");
  const [garminKeyInput, setGarminKeyInput] = useState("");
  const [geminiKeyInput, setGeminiKeyInput] = useState("");
  const [onboardStep, setOnboardStep] = useState<1 | 2 | 3>(1);
  const [busy, setBusy] = useState(false);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  const [chatInput, setChatInput] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [copiedPacket, setCopiedPacket] = useState(false);

  const [draftConfig, setDraftConfig] = useState<AthleteConfig | null>(null);

  async function reload() {
    const data = await invoke<BootstrapState>("get_bootstrap_state");
    setState(data);
    setDraftConfig(data.athlete_config);
  }

  useEffect(() => {
    void reload();
  }, []);

  if (!state || !draftConfig) {
    return <div className="main">Loading Splits Coach…</div>;
  }

  if (!state.has_garmin_key) {
    return (
      <div className="onboarding card">
        {onboardStep === 1 && (
          <>
            <h2>Welcome to Splits Coach</h2>
            <p className="muted">
              Your Garmin training, sleep, HRV, and deterministic readiness analyst — private on your PC.
            </p>
            <button className="primary" onClick={() => setOnboardStep(2)}>
              Continue
            </button>
          </>
        )}
        {onboardStep === 2 && (
          <>
            <h2>Connect Your Watch</h2>
            <p className="muted">Paste your Garmin Connect Personal API Key (stored in Windows Credential Manager).</p>
            <input
              type="password"
              placeholder="Personal API key"
              value={garminKeyInput}
              onChange={(e) => setGarminKeyInput(e.target.value)}
            />
            <div style={{ display: "flex", gap: 10, marginTop: 14 }}>
              <button className="secondary" onClick={() => setOnboardStep(1)}>Back</button>
              <button
                className="primary"
                disabled={!garminKeyInput.trim()}
                onClick={async () => {
                  await invoke("save_garmin_key", { key: garminKeyInput.trim() });
                  setGarminKeyInput("");
                  setOnboardStep(3);
                  await reload();
                }}
              >
                Connect
              </button>
            </div>
          </>
        )}
      </div>
    );
  }

  if (!state.settings.profile_onboarded && onboardStep === 3) {
    return (
      <div className="onboarding card">
        <h2>Athlete Profile & Guardrails</h2>
        <p className="muted">Configure your specs and guardrails in metric units, or skip for now.</p>
        <div className="form-row">
          <div>
            <label className="muted">Display Name</label>
            <input
              value={draftConfig.specs.displayName}
              onChange={(e) =>
                setDraftConfig({ ...draftConfig, specs: { ...draftConfig.specs, displayName: e.target.value } })
              }
            />
          </div>
          <div>
            <label className="muted">Weight (kg)</label>
            <input
              type="number"
              value={draftConfig.specs.weightKg ?? ""}
              onChange={(e) =>
                setDraftConfig({
                  ...draftConfig,
                  specs: { ...draftConfig.specs, weightKg: e.target.value ? Number(e.target.value) : null },
                })
              }
            />
          </div>
        </div>
        <div style={{ display: "flex", justifyContent: "flex-end", gap: 10 }}>
          <button
            className="secondary"
            onClick={async () => {
              await invoke("save_athlete_config", { config: state.athlete_config });
              await reload();
            }}
          >
            Skip for Now
          </button>
          <button
            className="primary"
            onClick={async () => {
              await invoke("save_athlete_config", { config: draftConfig });
              await reload();
            }}
          >
            Save & Continue
          </button>
        </div>
      </div>
    );
  }

  const r = state.readiness_state;
  const todayDay = state.days[0];
  const pillClass = !r ? "amber" : !r.gatekeeper.run_permitted ? "red" : r.gatekeeper.caution_flags.length > 0 ? "amber" : "green";

  async function handleCopyPacket() {
    const packet = await invoke<string>("build_coach_packet", { draftPrompt: chatInput });
    await navigator.clipboard.writeText(packet);
    setCopiedPacket(true);
    setTimeout(() => setCopiedPacket(false), 1500);
  }

  async function handleSendCoach() {
    const trimmed = chatInput.trim();
    if (!trimmed) return;
    setChatInput("");
    setMessages((prev) => [...prev, { role: "user", text: trimmed }]);
    setBusy(true);
    try {
      const res = await invoke<{ mode: string; text?: string; packet?: string }>("send_coach_message", {
        prompt: trimmed,
      });
      if (res.mode === "gemini_api") {
        setMessages((prev) => [...prev, { role: "assistant", text: res.text ?? "" }]);
      } else {
        if (res.packet) await navigator.clipboard.writeText(res.packet);
        setMessages((prev) => [
          ...prev,
          {
            role: "assistant",
            text: "No Gemini API key is configured. Your complete Markdown Coach Packet has been copied to the clipboard — paste it into any AI chat.",
          },
        ]);
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <h1>Splits Coach</h1>
        <button className={`nav-btn ${tab === "today" ? "active" : ""}`} onClick={() => setTab("today")}>
          <Sun size={16} /> Today
        </button>
        <button className={`nav-btn ${tab === "activities" ? "active" : ""}`} onClick={() => setTab("activities")}>
          <Activity size={16} /> Activities
        </button>
        <button className={`nav-btn ${tab === "coach" ? "active" : ""}`} onClick={() => setTab("coach")}>
          <MessageSquare size={16} /> Coach
        </button>
        <button className={`nav-btn ${tab === "settings" ? "active" : ""}`} onClick={() => setTab("settings")}>
          <Settings size={16} /> Settings
        </button>
        <div style={{ marginTop: "auto", padding: 8 }}>
          <button
            className="secondary"
            style={{ width: "100%", display: "flex", alignItems: "center", justifyContent: "center", gap: 6 }}
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await invoke("sync_garmin_now");
                await reload();
              } finally {
                setBusy(false);
              }
            }}
          >
            <RefreshCw size={14} /> Sync Watch
          </button>
        </div>
      </aside>

      <main className="main">
        {tab === "today" && (
          <>
            <div className="card">
              <div className="card-header">
                <div>
                  <strong>Coach Readiness</strong>
                  <div className="muted">{r ? `${r.date} · ${r.updated_by}` : "Not computed yet"}</div>
                </div>
                <button
                  className="secondary"
                  onClick={async () => {
                    await invoke("recompute_readiness");
                    await reload();
                  }}
                >
                  Recompute
                </button>
              </div>
              {r ? (
                <div>
                  <span className={`pill ${pillClass}`}>{r.prescription_summary.decision}</span>
                  <div style={{ marginTop: 10 }}>
                    <strong>{r.prescription_summary.distance_km.toFixed(1)} km</strong> · HR ceiling{" "}
                    <strong>{r.prescription_summary.hr_ceiling_bpm} bpm</strong> ·{" "}
                    {r.prescription_summary.shoe_platform}
                  </div>
                  <div className="muted" style={{ marginTop: 6 }}>
                    7-day rolling: {r.gatekeeper.rolling_7d_km.toFixed(1)} km ({r.gatekeeper.rolling_7d_runs} runs) ·
                    Remaining budget: {r.gatekeeper.remaining_weekly_budget_km.toFixed(1)} km
                  </div>
                </div>
              ) : (
                <div className="muted">Click Recompute to evaluate local readiness from your synced history.</div>
              )}
            </div>

            <div className="grid">
              <div className="card">
                <div className="muted">Resting Heart Rate</div>
                <div className="metric-val">{todayDay?.resting_hr ?? "–"} bpm</div>
              </div>
              <div className="card">
                <div className="muted">Overnight HRV</div>
                <div className="metric-val">{todayDay?.hrv_last_night ?? "–"} ms</div>
              </div>
              <div className="card">
                <div className="muted">Sleep Score</div>
                <div className="metric-val">{todayDay?.sleep_score ?? "–"}</div>
              </div>
              <div className="card">
                <div className="muted">Steps</div>
                <div className="metric-val">{todayDay?.steps?.toLocaleString() ?? "–"}</div>
              </div>
            </div>
          </>
        )}

        {tab === "activities" && (
          <div className="card">
            <h3>Recent Activities ({state.activities.length})</h3>
            {state.activities.length === 0 ? (
              <div className="muted">No activities synced yet.</div>
            ) : (
              state.activities.map((a) => (
                <div
                  key={a.garmin_id}
                  style={{ display: "flex", justifyContent: "space-between", padding: "10px 0", borderBottom: "1px solid var(--border)" }}
                >
                  <div>
                    <strong>{a.name || a.type_key}</strong>
                    <div className="muted">{a.start_iso.slice(0, 10)} · {a.type_key}</div>
                  </div>
                  <div style={{ textAlign: "right" }}>
                    <div>{(a.distance_m / 1000).toFixed(2)} km</div>
                    <div className="muted">{a.average_hr ? `${Math.round(a.average_hr)} bpm` : "–"}</div>
                  </div>
                </div>
              ))
            )}
          </div>
        )}

        {tab === "coach" && (
          <>
            <div className="card">
              <div className="card-header">
                <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                  <strong>Living Coach</strong>
                  <span className="pill green">
                    {state.engine_mode === "gemini_api" ? "Gemini API" : "Clipboard Packet"}
                  </span>
                </div>
                <button
                  className="secondary"
                  style={{ display: "flex", alignItems: "center", gap: 6 }}
                  onClick={handleCopyPacket}
                >
                  {copiedPacket ? <Check size={14} /> : <Clipboard size={14} />}
                  {copiedPacket ? "Copied" : "Copy Coach Packet"}
                </button>
              </div>
              <div className="chat-box">
                {messages.map((m, i) => (
                  <div key={i} className={`bubble ${m.role}`}>
                    {m.text}
                  </div>
                ))}
              </div>
              <div style={{ display: "flex", gap: 10, marginTop: 14 }}>
                <input
                  placeholder="Ask your coach…"
                  value={chatInput}
                  onChange={(e) => setChatInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void handleSendCoach();
                  }}
                />
                <button className="primary" disabled={busy} onClick={handleSendCoach}>
                  Send
                </button>
              </div>
            </div>
          </>
        )}

        {tab === "settings" && (
          <>
            <div className="card">
              <h3>Execution Mode</h3>
              <div style={{ display: "flex", gap: 10, marginBottom: 10 }}>
                {(["self_contained", "external_server"] as ExecutionMode[]).map((mode) => (
                  <button
                    key={mode}
                    className={state.settings.execution_mode === mode ? "primary" : "secondary"}
                    onClick={async () => {
                      await invoke("set_execution_mode", { mode });
                      await reload();
                    }}
                  >
                    {mode === "self_contained" ? "Self-Contained (On-PC)" : "External Server"}
                  </button>
                ))}
              </div>
              <div className="muted">Shared directory: {state.shared_dir}</div>
              <div style={{ display: "flex", gap: 10, marginTop: 10 }}>
                <button
                  className="secondary"
                  onClick={async () => {
                    const selected = await open({ directory: true, multiple: false });
                    if (typeof selected === "string") {
                      await invoke("set_custom_shared_dir", { path: selected });
                      await reload();
                    }
                  }}
                >
                  Choose Custom Folder…
                </button>
                {state.settings.custom_shared_coach_directory_path && (
                  <button
                    className="secondary"
                    onClick={async () => {
                      await invoke("set_custom_shared_dir", { path: null });
                      await reload();
                    }}
                  >
                    Reset to Default
                  </button>
                )}
              </div>
            </div>

            <div className="card">
              <h3>Gemini API Key (Windows Credential Manager)</h3>
              <div className="muted" style={{ marginBottom: 8 }}>
                Status: {state.has_gemini_key ? "Saved in Credential Manager" : "Not set (Clipboard Packet mode)"}
              </div>
              <div style={{ display: "flex", gap: 10 }}>
                <input
                  type="password"
                  placeholder="Gemini API key"
                  value={geminiKeyInput}
                  onChange={(e) => setGeminiKeyInput(e.target.value)}
                />
                <button
                  className="primary"
                  onClick={async () => {
                    await invoke("save_gemini_key", { key: geminiKeyInput });
                    setGeminiKeyInput("");
                    await reload();
                  }}
                >
                  Save Key
                </button>
              </div>
            </div>

            <div className="card">
              <h3>Athlete Profile & Guardrails</h3>
              <div className="form-row">
                <div>
                  <label className="muted">Display Name</label>
                  <input
                    value={draftConfig.specs.displayName}
                    onChange={(e) =>
                      setDraftConfig({ ...draftConfig, specs: { ...draftConfig.specs, displayName: e.target.value } })
                    }
                  />
                </div>
                <div>
                  <label className="muted">Goal Type</label>
                  <select
                    value={draftConfig.objective.goalType}
                    onChange={(e) =>
                      setDraftConfig({
                        ...draftConfig,
                        objective: { ...draftConfig.objective, goalType: e.target.value as GoalType },
                      })
                    }
                  >
                    <option value="general_fitness">General Fitness</option>
                    <option value="base_building">Base Building</option>
                    <option value="race">Target Race</option>
                    <option value="return_from_injury">Return from Injury</option>
                  </select>
                </div>
              </div>
              <div className="form-row">
                <div>
                  <label className="muted">VT1 Ceiling (bpm)</label>
                  <input
                    type="number"
                    value={draftConfig.specs.vt1CeilingBPM ?? ""}
                    onChange={(e) =>
                      setDraftConfig({
                        ...draftConfig,
                        specs: { ...draftConfig.specs, vt1CeilingBPM: e.target.value ? Number(e.target.value) : null },
                      })
                    }
                  />
                </div>
                <div>
                  <label className="muted">Min Rest Between Runs (h)</label>
                  <input
                    type="number"
                    value={draftConfig.medical.minRestHoursBetweenRuns ?? ""}
                    onChange={(e) =>
                      setDraftConfig({
                        ...draftConfig,
                        medical: {
                          ...draftConfig.medical,
                          minRestHoursBetweenRuns: e.target.value ? Number(e.target.value) : null,
                        },
                      })
                    }
                  />
                </div>
              </div>
              <button
                className="primary"
                onClick={async () => {
                  await invoke("save_athlete_config", { config: draftConfig });
                  setStatusMsg("Profile saved.");
                  await reload();
                }}
              >
                Save Profile
              </button>
              {statusMsg && <span className="muted" style={{ marginLeft: 12 }}>{statusMsg}</span>}
            </div>
          </>
        )}
      </main>
    </div>
  );
}
