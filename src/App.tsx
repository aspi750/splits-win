import { useEffect, useMemo, useState, type ReactNode } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import {
  Activity,
  ArrowLeft,
  Check,
  ChevronDown,
  Clipboard,
  Download,
  FileText,
  MessageSquare,
  RefreshCw,
  Settings,
  Sparkles,
  SquarePen,
  Sun,
  Trash2,
  Upload,
} from "lucide-react";
import { OnboardingWizard } from "./components/OnboardingWizard";
import type {
  ActivityDetailPayload,
  ActivityRecord,
  AthleteConfig,
  BootstrapState,
  CoachUpdateProposal,
  ExecutionMode,
  FirstUseStage,
  FitTrackPoint,
  GoalType,
  ModelSettings,
  PostRunContext,
  ThinkingLevel,
} from "./types";

type NavTab = "today" | "activities" | "coach" | "settings";

interface ChatMessage {
  role: "user" | "assistant";
  text: string;
  proposals?: CoachUpdateProposal[];
}

const GEMINI_MODEL_OPTIONS = [
  { id: "gemini-4-hydrogen", label: "Gemini 4 Hydrogen", short: "Hydrogen" },
  { id: "gemini-3.1-pro-preview", label: "Gemini 3.1 Pro", short: "3.1 Pro" },
  { id: "gemini-2.5-pro", label: "Gemini 2.5 Pro", short: "2.5 Pro" },
  { id: "gemini-2.5-flash", label: "Gemini 2.5 Flash", short: "2.5 Flash" },
];

function shortModelName(id: string): string {
  return GEMINI_MODEL_OPTIONS.find((m) => m.id === id)?.short ?? id;
}

function thinkingLabel(level: ThinkingLevel): string {
  switch (level) {
    case "LOW":
      return "Low";
    case "HIGH":
      return "High";
    default:
      return "Medium";
  }
}

function renderInlineMarkdown(text: string): ReactNode[] {
  const tokens = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g);
  return tokens.map((tok, idx) => {
    if (tok.startsWith("**") && tok.endsWith("**") && tok.length > 4) {
      return <strong key={idx}>{tok.slice(2, -2)}</strong>;
    }
    if (tok.startsWith("`") && tok.endsWith("`") && tok.length > 2) {
      return <code key={idx}>{tok.slice(1, -1)}</code>;
    }
    return tok;
  });
}

function MarkdownPreview({ markdown }: { markdown: string }) {
  const lines = markdown.replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;

  while (i < lines.length) {
    const raw = lines[i];
    const trimmed = raw.trim();

    if (!trimmed) {
      i += 1;
      continue;
    }

    if (trimmed.startsWith("### ")) {
      blocks.push(<h3 key={i}>{renderInlineMarkdown(trimmed.slice(4))}</h3>);
      i += 1;
      continue;
    }
    if (trimmed.startsWith("## ")) {
      blocks.push(<h2 key={i}>{renderInlineMarkdown(trimmed.slice(3))}</h2>);
      i += 1;
      continue;
    }
    if (trimmed.startsWith("# ")) {
      blocks.push(<h1 key={i}>{renderInlineMarkdown(trimmed.slice(2))}</h1>);
      i += 1;
      continue;
    }

    if (trimmed.startsWith("> ")) {
      const quoteLines: string[] = [];
      while (i < lines.length && lines[i].trim().startsWith("> ")) {
        quoteLines.push(lines[i].trim().slice(2));
        i += 1;
      }
      blocks.push(
        <blockquote key={`q-${i}`}>
          {quoteLines.map((ql, qi) => (
            <p key={qi}>{renderInlineMarkdown(ql)}</p>
          ))}
        </blockquote>
      );
      continue;
    }

    if (trimmed.startsWith("- ") || trimmed.startsWith("* ")) {
      const items: string[] = [];
      while (i < lines.length && (lines[i].trim().startsWith("- ") || lines[i].trim().startsWith("* "))) {
        items.push(lines[i].trim().slice(2));
        i += 1;
      }
      blocks.push(
        <ul key={`ul-${i}`}>
          {items.map((it, ii) => (
            <li key={ii}>{renderInlineMarkdown(it)}</li>
          ))}
        </ul>
      );
      continue;
    }

    if (trimmed.startsWith("|")) {
      const tableLines: string[] = [];
      while (i < lines.length && lines[i].trim().startsWith("|")) {
        tableLines.push(lines[i].trim());
        i += 1;
      }
      const parseRow = (row: string) =>
        row
          .replace(/^\|/, "")
          .replace(/\|$/, "")
          .split("|")
          .map((c) => c.trim());
      const rows = tableLines
        .filter((r) => !/^\|?[\s:-]+(\|[\s:-]+)+\|?$/.test(r))
        .map(parseRow);
      if (rows.length > 0) {
        const [header, ...body] = rows;
        blocks.push(
          <table key={`tbl-${i}`}>
            <thead>
              <tr>
                {header.map((cell, ci) => (
                  <th key={ci}>{renderInlineMarkdown(cell)}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {body.map((row, ri) => (
                <tr key={ri}>
                  {row.map((cell, ci) => (
                    <td key={ci}>{renderInlineMarkdown(cell)}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        );
        continue;
      }
    }

    blocks.push(<p key={i}>{renderInlineMarkdown(trimmed)}</p>);
    i += 1;
  }

  return <div className="md-preview">{blocks}</div>;
}

function formatPace(secPerKm: number | null | undefined): string {
  if (!secPerKm || !Number.isFinite(secPerKm) || secPerKm <= 0) return "–";
  const total = Math.round(secPerKm);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}/km`;
}

function formatDuration(sec: number): string {
  const total = Math.max(0, Math.round(sec));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}

export default function App() {
  const [state, setState] = useState<BootstrapState | null>(null);
  const [tab, setTab] = useState<NavTab>("today");
  const [garminKeyInput, setGarminKeyInput] = useState("");
  const [geminiKeyInput, setGeminiKeyInput] = useState("");
  const [onboardStep, setOnboardStep] = useState<1 | 2 | 3>(1);
  const [busy, setBusy] = useState(false);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);
  const [coachError, setCoachError] = useState<string | null>(null);
  const [syncProgressMsg, setSyncProgressMsg] = useState<string | null>(null);

  const [chatInput, setChatInput] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [liveToolLabel, setLiveToolLabel] = useState<string | null>(null);
  const [copiedPacket, setCopiedPacket] = useState(false);
  const [showModelPopover, setShowModelPopover] = useState(false);

  const [selectedActivity, setSelectedActivity] = useState<ActivityDetailPayload | null>(null);
  const [loadingDetail, setLoadingDetail] = useState(false);

  const [draftConfig, setDraftConfig] = useState<AthleteConfig | null>(null);
  const [showOnboarding, setShowOnboarding] = useState(false);
  const [ledgerNotice, setLedgerNotice] = useState<string | null>(null);
  const [paceFloorInput, setPaceFloorInput] = useState("");

  const [appVersion, setAppVersion] = useState<string>("0.2.2-beta");
  const [availableUpdate, setAvailableUpdate] = useState<Update | null>(null);
  const [updateStatus, setUpdateStatus] = useState<string | null>(null);
  const [updateChecking, setUpdateChecking] = useState(false);
  const [updateInstalling, setUpdateInstalling] = useState(false);
  const [updateReadyToRestart, setUpdateReadyToRestart] = useState(false);
  const [updateProgress, setUpdateProgress] = useState<{ downloaded: number; total: number | null } | null>(null);

  async function reload() {
    const data = await invoke<BootstrapState>("get_bootstrap_state");
    setState(data);
    setDraftConfig(data.athlete_config);
    const sec = data.athlete_config.medical.minPaceSecPerKm;
    setPaceFloorInput(
      sec && sec > 0 ? `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, "0")}` : ""
    );
  }

  async function runUpdateCheck(silent: boolean) {
    if (!silent) {
      setUpdateChecking(true);
      setUpdateStatus("Checking…");
    }
    try {
      const update = await check();
      if (update) {
        setAvailableUpdate(update);
        setUpdateStatus(`v${update.version} available`);
      } else if (!silent) {
        setAvailableUpdate(null);
        setUpdateStatus("Up to date");
      }
    } catch (err) {
      if (!silent) {
        setUpdateStatus(`Update check failed: ${err instanceof Error ? err.message : String(err)}`);
      }
    } finally {
      if (!silent) {
        setUpdateChecking(false);
      }
    }
  }

  async function handleInstallUpdate() {
    if (!availableUpdate) return;
    setUpdateInstalling(true);
    setUpdateProgress({ downloaded: 0, total: null });
    setUpdateStatus(`Downloading v${availableUpdate.version}…`);
    let downloaded = 0;
    let total: number | null = null;
    try {
      await availableUpdate.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? null;
          setUpdateProgress({ downloaded: 0, total });
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
          setUpdateProgress({ downloaded, total });
        } else if (event.event === "Finished") {
          setUpdateProgress({ downloaded: total ?? downloaded, total });
        }
      });
      setUpdateReadyToRestart(true);
      setUpdateStatus(`v${availableUpdate.version} installed. Restart to apply.`);
    } catch (err) {
      setUpdateStatus(`Install failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setUpdateInstalling(false);
    }
  }

  useEffect(() => {
    void reload();
    void getVersion().then(setAppVersion).catch(() => {});
    void runUpdateCheck(true);
  }, []);

  useEffect(() => {
    const unlistenDelta = listen<{ delta: string }>("coach:delta", (event) => {
      const delta = event.payload?.delta ?? "";
      if (!delta) return;
      setLiveToolLabel(null);
      setMessages((prev) => {
        if (prev.length === 0) return prev;
        const next = [...prev];
        const last = next[next.length - 1];
        if (last.role === "assistant") {
          next[next.length - 1] = { ...last, text: last.text + delta };
        }
        return next;
      });
    });

    const unlistenTool = listen<{ tool: string; label: string; active: boolean }>(
      "coach:tool_status",
      (event) => {
        setLiveToolLabel(event.payload?.active ? event.payload.label : null);
      }
    );

    return () => {
      void unlistenDelta.then((f) => f());
      void unlistenTool.then((f) => f());
    };
  }, []);

  const weeklyBars = useMemo(() => {
    if (!state) return [];
    const now = new Date();
    const out: { label: string; km: number; runs: number }[] = [];
    for (let i = 6; i >= 0; i--) {
      const d = new Date(now);
      d.setDate(now.getDate() - i);
      const key = d.toISOString().slice(0, 10);
      const dayActs = state.activities.filter(
        (a) => a.start_iso.slice(0, 10) === key && a.type_key.toLowerCase().includes("run")
      );
      out.push({
        label: d.toLocaleDateString(undefined, { weekday: "short" }),
        km: dayActs.reduce((acc, a) => acc + a.distance_m / 1000, 0),
        runs: dayActs.length,
      });
    }
    return out;
  }, [state]);

  if (!state || !draftConfig) {
    return <div className="main">Loading Splits Coach…</div>;
  }

  const shouldShowOnboarding =
    showOnboarding || state.user_profile?.onboarding_completed === false;

  if (shouldShowOnboarding) {
    const defaultProfile = state.user_profile ?? {
      athlete_type: "runner" as const,
      onboarding_completed: false,
      runner_config: {
        primary_distance: null,
        hr_first_governor: true,
        pace_floor_sec_per_km: null,
        z2_range_bpm: null,
        z3_range_bpm: null,
        lthr_bpm: null,
      },
      hyrox_config: {
        division: null,
        watch_logging_mode: null,
        target_total_time_sec: null,
        fresh_1km_pace_sec: null,
        compromised_1km_target_sec: null,
        weak_stations: [],
      },
    };
    return (
      <OnboardingWizard
        initialConfig={draftConfig}
        initialProfile={defaultProfile}
        initialModelSettings={state.model_settings}
        initialFirstUseState={state.first_use_state}
        hasGarminKey={state.has_garmin_key}
        hasGeminiKey={state.has_gemini_key}
        canDismiss={Boolean(state.user_profile?.onboarding_completed)}
        onDismiss={() => setShowOnboarding(false)}
        onComplete={async () => {
          setShowOnboarding(false);
          setMessages([]);
          setCoachError(null);
          await reload();
          setTab("coach");
        }}
      />
    );
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
  const latestRun = state.activities.find((a) => a.type_key.toLowerCase().includes("run")) ?? state.activities[0];
  const pillClass = !r ? "amber" : !r.gatekeeper.run_permitted ? "red" : r.gatekeeper.caution_flags.length > 0 ? "amber" : "green";

  const modelSettings: ModelSettings = state.model_settings ?? {
    global_model_id: "gemini-4-hydrogen",
    global_thinking_level: "MEDIUM",
    coach_model_override_id: null,
    coach_thinking_override: null,
  };
  const firstUseState = state.first_use_state ?? {
    stage: state.needs_initial_coach_setup ? ("intake" as FirstUseStage) : ("active" as FirstUseStage),
    auto_calibrate_after_first_sync: true,
    queued_intake_files: [],
    history_summary_markdown: null,
    updated_at: "",
  };
  const firstUseStage: FirstUseStage = firstUseState.stage;
  const isGuidedFirstUse = firstUseStage !== "active";

  const hasCoachOverride =
    modelSettings.coach_model_override_id != null || modelSettings.coach_thinking_override != null;
  const effectiveCoachModelId =
    modelSettings.coach_model_override_id ?? modelSettings.global_model_id;
  const effectiveCoachThinking =
    modelSettings.coach_thinking_override ?? modelSettings.global_thinking_level;
  const coachPillText = hasCoachOverride
    ? `Override: ${shortModelName(effectiveCoachModelId)} ${thinkingLabel(effectiveCoachThinking)}`
    : `Global (${shortModelName(modelSettings.global_model_id)} ${thinkingLabel(modelSettings.global_thinking_level)})`;

  async function handleCopyPacket() {
    const packet = await invoke<string>("build_coach_packet", { draftPrompt: chatInput });
    await navigator.clipboard.writeText(packet);
    setCopiedPacket(true);
    setTimeout(() => setCopiedPacket(false), 1500);
  }

  async function sendPrompt(promptText: string) {
    const trimmed = promptText.trim();
    if (!trimmed || busy) return;
    setCoachError(null);
    setMessages((prev) => [
      ...prev,
      { role: "user", text: trimmed },
      { role: "assistant", text: "" },
    ]);
    setBusy(true);
    setLiveToolLabel(null);
    try {
      const res = await invoke<{
        mode: string;
        text?: string;
        packet?: string;
        proposals?: CoachUpdateProposal[];
      }>("send_coach_message", { prompt: trimmed });

      setMessages((prev) => {
        const next = [...prev];
        const idx = next.length - 1;
        if (idx >= 0 && next[idx].role === "assistant") {
          if (res.mode === "gemini_api") {
            next[idx] = {
              role: "assistant",
              text: res.text || next[idx].text,
              proposals: res.proposals ?? [],
            };
          } else {
            if (res.packet) void navigator.clipboard.writeText(res.packet);
            next[idx] = {
              role: "assistant",
              text: "No Gemini API key is configured. Your complete Markdown Coach Packet has been copied to the clipboard — paste it into any AI chat.",
            };
          }
        }
        return next;
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setCoachError(msg);
      setMessages((prev) => {
        const next = [...prev];
        const idx = next.length - 1;
        if (idx >= 0 && next[idx].role === "assistant" && !next[idx].text) {
          next[idx] = { role: "assistant", text: `Error: ${msg}` };
        }
        return next;
      });
    } finally {
      setBusy(false);
      setLiveToolLabel(null);
    }
  }

  async function handleSendCoach() {
    const text = chatInput;
    setChatInput("");
    await sendPrompt(text);
  }

  async function analyzeInCoach(activity: ActivityRecord) {
    setTab("coach");
    const prompt = `Analyze my activity "${activity.name || activity.type_key}" on ${activity.start_iso.slice(0, 10)} (activity_id: ${activity.garmin_id}) in detail. Fetch its .fit splits and telemetry via get_activity_detail, evaluate per-km pacing, cardiac decoupling / HR drift, Garmin Connect notes if any, and interpret it against my active plan and readiness context.`;
    await sendPrompt(prompt);
  }

  async function openActivityDetail(activity: ActivityRecord) {
    setLoadingDetail(true);
    try {
      const detail = await invoke<ActivityDetailPayload>("get_activity_detail", {
        activityId: activity.garmin_id,
      });
      setSelectedActivity(detail);
    } finally {
      setLoadingDetail(false);
    }
  }

  async function handleStage1UploadFiles() {
    const selected = await open({
      multiple: true,
      filters: [{ name: "Intake Docs (.pdf, .fit, .txt, .md)", extensions: ["pdf", "fit", "txt", "md"] }],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    setBusy(true);
    setCoachError(null);
    try {
      for (const p of paths) {
        if (typeof p === "string" && p.trim()) {
          await invoke("queue_intake_file", { sourcePath: p });
        }
      }
      await reload();
    } catch (err) {
      setCoachError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleRemoveQueuedFile(fileId: string) {
    setBusy(true);
    try {
      await invoke("remove_intake_file", { fileId });
      await reload();
    } finally {
      setBusy(false);
    }
  }

  async function handleContinueToProfileSynthesis() {
    setBusy(true);
    setCoachError(null);
    try {
      await invoke("advance_first_use_stage", { stage: "profile_approval" });
      await reload();
      const count = firstUseState.queued_intake_files.length;
      const fileSummary =
        count > 0
          ? `I uploaded ${count} file(s) for intake (${firstUseState.queued_intake_files.map((f) => f.file_name).join(", ")}). `
          : "I have no additional files to upload right now. ";
      const prompt = `${fileSummary}Please review my onboarding profile, ask about or clarify my advanced guardrails (HR-First Governor, Hard Pace Floor, rest spacing), and prepare ONE consolidated COACH_PROFILE.md proposal for my approval.`;
      setBusy(false);
      await sendPrompt(prompt);
    } catch (err) {
      setCoachError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  }

  async function handleRunHistorySyncStage(runGarminSync: boolean) {
    setBusy(true);
    setCoachError(null);
    setSyncProgressMsg(
      runGarminSync
        ? "Downloading Garmin lifetime activity history, 365-day wellness metrics, and auto-calibrating baselines…"
        : "Analyzing local training history and calibrating baselines…"
    );
    try {
      await invoke("complete_history_sync_stage", { runGarminSync });
      setSyncProgressMsg(null);
      await reload();
    } catch (err) {
      setSyncProgressMsg(null);
      setCoachError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleRequestConsolidatedPlan(blockPreference: string) {
    const trimmed = blockPreference.trim();
    const prefText = trimmed
      ? `My preferred block structure / scheduling constraints: ${trimmed}. `
      : "Use a balanced progressive block structure suited to my recent history. ";
    await sendPrompt(
      `${prefText}Review my synced lifetime and 365-day history summary and propose ONE consolidated TRAINING_PLAN.md (with structured plan_json) for my approval.`
    );
  }

  async function handleProposalDecision(msgIdx: number, propId: string, approve: boolean) {
    const targetProp = messages[msgIdx]?.proposals?.find((p) => p.id === propId);
    if (!targetProp) return;
    if (approve) {
      await invoke("apply_coach_proposal", { proposal: targetProp });
      await reload();
    }
    setMessages((prev) =>
      prev.map((m, i) =>
        i !== msgIdx
          ? m
          : {
              ...m,
              proposals: m.proposals?.map((p) =>
                p.id === propId ? { ...p, status: approve ? "applied" : "dismissed" } : p
              ),
            }
      )
    );
  }

  const stageSteps: { id: FirstUseStage; label: string }[] = [
    { id: "intake", label: "1. File Intake" },
    { id: "profile_approval", label: "2. Profile Approval" },
    { id: "history_sync", label: "3. History Sync" },
    { id: "plan_approval", label: "4. Plan Approval" },
  ];
  const currentStageIdx = stageSteps.findIndex((s) => s.id === firstUseStage);

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <h1>Splits Coach</h1>
        <button className={`nav-btn ${tab === "today" ? "active" : ""}`} onClick={() => setTab("today")}>
          <Sun size={16} /> Today
        </button>
        <button
          className={`nav-btn ${tab === "activities" ? "active" : ""}`}
          onClick={() => {
            setSelectedActivity(null);
            setTab("activities");
          }}
        >
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
        {availableUpdate && (
          <div
            style={{
              marginBottom: 14,
              padding: "10px 14px",
              borderRadius: 10,
              border: "1px solid var(--border)",
              background: "var(--card)",
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              gap: 12,
            }}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <span className="pill green">Update v{availableUpdate.version} available</span>
              <span className="muted" style={{ fontSize: 13 }}>
                {updateReadyToRestart
                  ? "Ready to restart and finish installing."
                  : "A signed update is ready to install."}
              </span>
            </div>
            {updateReadyToRestart ? (
              <button className="primary" onClick={() => void relaunch()}>
                Restart Now
              </button>
            ) : (
              <button className="secondary" onClick={() => setTab("settings")}>
                View Update
              </button>
            )}
          </div>
        )}
        {tab === "today" && (
          <>
            <div className="card">
              <div className="card-header">
                <div>
                  <strong>Coach Readiness</strong>
                  <div className="muted">{r ? `${r.date} · ${r.updated_by}` : "Not computed yet"}</div>
                </div>
                <div style={{ display: "flex", gap: 8 }}>
                  {latestRun && (
                    <button
                      className="secondary"
                      style={{ display: "flex", alignItems: "center", gap: 5 }}
                      onClick={() => void analyzeInCoach(latestRun)}
                    >
                      <Sparkles size={14} /> Analyze Latest Run
                    </button>
                  )}
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

            <div className="card">
              <div className="card-header">
                <strong>7-Day Running Volume</strong>
                <span className="muted">
                  {weeklyBars.reduce((s, b) => s + b.km, 0).toFixed(1)} km ·{" "}
                  {weeklyBars.reduce((s, b) => s + b.runs, 0)} runs
                </span>
              </div>
              <WeeklyVolumeChart bars={weeklyBars} />
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

        {tab === "activities" && !selectedActivity && (
          <div className="card">
            <h3>Recent Activities ({state.activities.length})</h3>
            {loadingDetail && <div className="muted">Loading .fit track…</div>}
            {state.activities.length === 0 ? (
              <div className="muted">No activities synced yet.</div>
            ) : (
              state.activities.map((a) => (
                <div
                  key={a.garmin_id}
                  onClick={() => void openActivityDetail(a)}
                  style={{
                    display: "flex",
                    justifyContent: "space-between",
                    padding: "10px 6px",
                    borderBottom: "1px solid var(--border)",
                    cursor: "pointer",
                  }}
                >
                  <div>
                    <strong>{a.name || a.type_key}</strong>
                    <div className="muted">
                      {a.start_iso.slice(0, 10)} · {a.type_key}
                      {a.garmin_notes ? " · Notes" : ""}
                    </div>
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

        {tab === "activities" && selectedActivity && (
          <ActivityDetailView
            detail={selectedActivity}
            onBack={() => setSelectedActivity(null)}
            onAnalyze={() => void analyzeInCoach(selectedActivity.activity)}
            onToggleHyrox={async (isHyrox: boolean) => {
              const updated = await invoke<ActivityDetailPayload>("reclassify_activity_as_hyrox", {
                activityId: selectedActivity.activity.garmin_id,
                isHyrox,
              });
              setSelectedActivity(updated);
              await reload();
            }}
            onActivityUpdated={async (updatedActivity: ActivityRecord) => {
              setSelectedActivity((prev) =>
                prev && prev.activity.garmin_id === updatedActivity.garmin_id
                  ? { ...prev, activity: updatedActivity }
                  : prev
              );
              await reload();
            }}
          />
        )}

        {tab === "coach" && (
          <div className="card">
            <div className="card-header">
              <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
                <strong>Living Coach</strong>
                <span className="pill green">
                  {state.engine_mode === "gemini_api" ? "Gemini API" : "Clipboard Packet"}
                </span>

                <div className="model-pill-wrap">
                  <button
                    type="button"
                    className="model-pill-btn"
                    onClick={() => setShowModelPopover((v) => !v)}
                  >
                    <span>{coachPillText}</span>
                    <ChevronDown size={13} />
                  </button>
                  {showModelPopover && (
                    <div className="model-popover">
                      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
                        <strong style={{ fontSize: 13 }}>Coach Tab Model</strong>
                        {hasCoachOverride && (
                          <button
                            type="button"
                            className="secondary"
                            style={{ padding: "3px 8px", fontSize: 11 }}
                            onClick={async () => {
                              await invoke("save_coach_model_override", {
                                modelId: null,
                                thinkingLevel: null,
                              });
                              await reload();
                              setShowModelPopover(false);
                            }}
                          >
                            Follow Global
                          </button>
                        )}
                      </div>
                      <label className="muted" style={{ fontSize: 12 }}>Model</label>
                      <select
                        style={{ marginTop: 4, marginBottom: 8 }}
                        value={modelSettings.coach_model_override_id ?? "__global__"}
                        onChange={async (e) => {
                          const val = e.target.value;
                          await invoke("save_coach_model_override", {
                            modelId: val === "__global__" ? null : val,
                            thinkingLevel: modelSettings.coach_thinking_override,
                          });
                          await reload();
                        }}
                      >
                        <option value="__global__">
                          Follow Global ({shortModelName(modelSettings.global_model_id)})
                        </option>
                        {GEMINI_MODEL_OPTIONS.map((opt) => (
                          <option key={opt.id} value={opt.id}>
                            {opt.label}
                          </option>
                        ))}
                      </select>
                      <label className="muted" style={{ fontSize: 12 }}>Thinking Level</label>
                      <select
                        style={{ marginTop: 4, marginBottom: 8 }}
                        value={modelSettings.coach_thinking_override ?? "__global__"}
                        onChange={async (e) => {
                          const val = e.target.value;
                          await invoke("save_coach_model_override", {
                            modelId: modelSettings.coach_model_override_id,
                            thinkingLevel: val === "__global__" ? null : val,
                          });
                          await reload();
                        }}
                      >
                        <option value="__global__">
                          Follow Global ({thinkingLabel(modelSettings.global_thinking_level)})
                        </option>
                        <option value="LOW">Low</option>
                        <option value="MEDIUM">Medium</option>
                        <option value="HIGH">High</option>
                      </select>
                      <div style={{ display: "flex", justifyContent: "flex-end" }}>
                        <button
                          type="button"
                          className="primary"
                          style={{ padding: "5px 10px", fontSize: 12 }}
                          onClick={() => setShowModelPopover(false)}
                        >
                          Done
                        </button>
                      </div>
                    </div>
                  )}
                </div>

                {ledgerNotice && <span className="pill">{ledgerNotice}</span>}
              </div>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                <button
                  className="secondary"
                  style={{ display: "flex", alignItems: "center", gap: 6 }}
                  onClick={async () => {
                    const selected = await open({
                      multiple: false,
                      filters: [{ name: "Training Ledger", extensions: ["txt", "md"] }],
                    });
                    if (typeof selected === "string" && selected) {
                      try {
                        const lines = await invoke<number>("import_training_ledger", { sourcePath: selected });
                        setLedgerNotice(`Imported ledger (${lines} lines)`);
                      } catch (err) {
                        setLedgerNotice(`Import error: ${err instanceof Error ? err.message : String(err)}`);
                      }
                    }
                  }}
                >
                  <Upload size={14} /> Import Ledger
                </button>
                <button
                  className="secondary"
                  style={{ display: "flex", alignItems: "center", gap: 6 }}
                  onClick={async () => {
                    const target = await save({
                      defaultPath: "ASPI_Training_Log.txt",
                      filters: [{ name: "Training Ledger", extensions: ["txt", "md"] }],
                    });
                    if (target) {
                      try {
                        const lines = await invoke<number>("export_training_ledger", { targetPath: target });
                        setLedgerNotice(`Exported ledger (${lines} lines)`);
                      } catch (err) {
                        setLedgerNotice(`Export error: ${err instanceof Error ? err.message : String(err)}`);
                      }
                    }
                  }}
                >
                  <Download size={14} /> Export Ledger
                </button>
                <button
                  className="secondary"
                  style={{ display: "flex", alignItems: "center", gap: 6 }}
                  disabled={busy || messages.length === 0}
                  onClick={() => setMessages([])}
                >
                  <SquarePen size={14} /> New Chat
                </button>
                <button
                  className="secondary"
                  style={{ display: "flex", alignItems: "center", gap: 6 }}
                  onClick={handleCopyPacket}
                >
                  {copiedPacket ? <Check size={14} /> : <Clipboard size={14} />}
                  {copiedPacket ? "Copied" : "Copy Coach Packet"}
                </button>
              </div>
            </div>

            {isGuidedFirstUse && (
              <div style={{ marginBottom: 14 }}>
                <div className="stage-stepper">
                  {stageSteps.map((s, idx) => (
                    <div
                      key={s.id}
                      className={`stage-step ${
                        idx === currentStageIdx ? "active" : idx < currentStageIdx ? "done" : ""
                      }`}
                    >
                      {s.label}
                    </div>
                  ))}
                </div>

                {firstUseStage === "intake" && (
                  <div
                    style={{
                      padding: 14,
                      borderRadius: 10,
                      border: "1px solid var(--accent-border)",
                      background: "var(--accent-soft-bg)",
                    }}
                  >
                    <strong>Stage 1 · Optional Intake Files Before Setup</strong>
                    <p className="muted" style={{ fontSize: 13, margin: "6px 0 10px" }}>
                      Before we build your initial athlete profile, would you like to upload any relevant files — such as a
                      lab stress test (<code>.pdf</code>), a Garmin stress/lactate test (<code>.fit</code>), or medical notes?
                    </p>
                    {firstUseState.queued_intake_files.length > 0 && (
                      <div style={{ display: "flex", flexDirection: "column", gap: 6, marginBottom: 10 }}>
                        {firstUseState.queued_intake_files.map((f) => (
                          <div
                            key={f.id}
                            style={{
                              display: "flex",
                              justifyContent: "space-between",
                              alignItems: "center",
                              padding: "6px 10px",
                              borderRadius: 6,
                              background: "var(--card)",
                              border: "1px solid var(--border)",
                              fontSize: 12,
                            }}
                          >
                            <span style={{ display: "flex", alignItems: "center", gap: 6 }}>
                              <FileText size={13} />
                              <strong>{f.file_name}</strong>
                              <span className="muted">({f.kind})</span>
                            </span>
                            <button
                              type="button"
                              className="secondary"
                              style={{ padding: "3px 8px", fontSize: 11 }}
                              disabled={busy}
                              onClick={() => void handleRemoveQueuedFile(f.id)}
                            >
                              <Trash2 size={12} />
                            </button>
                          </div>
                        ))}
                      </div>
                    )}
                    <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                      <button className="secondary" disabled={busy} onClick={() => void handleStage1UploadFiles()}>
                        <Upload size={13} style={{ marginRight: 5, verticalAlign: "middle" }} />
                        Upload File(s)
                      </button>
                      <button
                        className="primary"
                        disabled={busy || state.engine_mode !== "gemini_api"}
                        onClick={() => void handleContinueToProfileSynthesis()}
                      >
                        {firstUseState.queued_intake_files.length > 0
                          ? `Continue with ${firstUseState.queued_intake_files.length} file(s)`
                          : "Continue without files"}
                      </button>
                    </div>
                  </div>
                )}

                {firstUseStage === "profile_approval" && (
                  <div
                    style={{
                      padding: 12,
                      borderRadius: 10,
                      border: "1px solid var(--accent-border)",
                      background: "var(--accent-soft-bg)",
                      fontSize: 13,
                    }}
                  >
                    <strong>Stage 2 · Initial Profile &amp; Guardrail Clarification</strong>
                    <div className="muted" style={{ fontSize: 12, marginTop: 4 }}>
                      Review your Coach&apos;s consolidated <code>COACH_PROFILE.md</code> proposal below (including HR-First Governor and Pace Floor discussion). Click{" "}
                      <strong>Approve Initial Profile</strong> when ready to proceed to Garmin History Sync.
                    </div>
                  </div>
                )}

                {firstUseStage === "history_sync" && (
                  <div
                    style={{
                      padding: 14,
                      borderRadius: 10,
                      border: "1px solid var(--accent-border)",
                      background: "var(--accent-soft-bg)",
                    }}
                  >
                    <strong>Stage 3 · Full Garmin History Sync &amp; Training Audit</strong>
                    <p className="muted" style={{ fontSize: 13, margin: "6px 0 10px" }}>
                      Your initial profile is approved. Sync your full Garmin history (plus 365-day wellness &amp; baseline calibration) to analyze your lifetime and yearly training blocks before generating your plan.
                    </p>
                    {syncProgressMsg && (
                      <div className="pill amber" style={{ marginBottom: 10 }}>
                        {syncProgressMsg}
                      </div>
                    )}
                    <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                      <button
                        className="primary"
                        disabled={busy}
                        onClick={() => void handleRunHistorySyncStage(true)}
                      >
                        {busy ? "Syncing History…" : "Download Garmin History & Summarize"}
                      </button>
                      <button
                        className="secondary"
                        disabled={busy}
                        onClick={() => void handleRunHistorySyncStage(false)}
                      >
                        Use Existing Local History
                      </button>
                    </div>
                  </div>
                )}

                {firstUseStage === "plan_approval" && (
                  <div
                    style={{
                      padding: 14,
                      borderRadius: 10,
                      border: "1px solid var(--accent-border)",
                      background: "var(--bg)",
                    }}
                  >
                    <strong>Stage 4 · Confirm Block Structure &amp; Approve Training Plan</strong>
                    {firstUseState.history_summary_markdown && (
                      <div style={{ marginTop: 8, marginBottom: 10 }}>
                        <MarkdownPreview markdown={firstUseState.history_summary_markdown} />
                      </div>
                    )}
                    <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                      <button
                        className="primary"
                        disabled={busy || state.engine_mode !== "gemini_api"}
                        onClick={() =>
                          void handleRequestConsolidatedPlan("3 weeks build + 1 week deload, polarized Z2 + 1 quality session")
                        }
                      >
                        Generate Consolidated Training Plan
                      </button>
                      <button
                        className="secondary"
                        disabled={busy}
                        onClick={async () => {
                          await invoke("advance_first_use_stage", { stage: "active" });
                          await reload();
                        }}
                      >
                        Mark Coach Active
                      </button>
                    </div>
                  </div>
                )}
              </div>
            )}

            {coachError && (
              <div className="pill red" style={{ marginBottom: 12, display: "block" }}>
                {coachError}
              </div>
            )}

            {messages.length === 0 && !isGuidedFirstUse && (
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 12 }}>
                {(state.user_profile?.athlete_type === "hyrox" ||
                state.user_profile?.athlete_type === "hybrid_both"
                  ? [
                      {
                        label: "Analyze compromised 1km run decay",
                        prompt:
                          "Analyze my latest HYROX / hybrid session's compromised 1km run decay and clean running efficiency.",
                      },
                      {
                        label: "Audit Run vs. Station split ratio",
                        prompt:
                          "Audit my Run vs. Station split ratio and Roxzone transitions from my latest HYROX sim.",
                      },
                      {
                        label: "Target weakest station session",
                        prompt:
                          "Design a targeted session for my weakest HYROX stations without compromising my aerobic running base.",
                      },
                      {
                        label: "Today's readiness check",
                        prompt: "Check today's gatekeeper readiness, recovery status, and recommended session.",
                      },
                    ]
                  : [
                      {
                        label: "Analyze last run",
                        prompt: "Analyze my latest run in detail, including splits, heart rate drift, and pacing.",
                      },
                      {
                        label: "Today's readiness check",
                        prompt: "Check today's gatekeeper readiness, recovery status, and recommended session.",
                      },
                      {
                        label: "Lifetime & 365d history",
                        prompt: "Review my lifetime training overview and trailing 365-day block audit.",
                      },
                    ]
                ).map((q) => (
                  <button
                    key={q.label}
                    className="secondary"
                    disabled={busy}
                    onClick={() => setChatInput(q.prompt)}
                  >
                    {q.label}
                  </button>
                ))}
              </div>
            )}

            {liveToolLabel && (
              <div className="pill amber" style={{ marginBottom: 12 }}>
                {liveToolLabel}
              </div>
            )}

            <div className="chat-box">
              {messages.map((m, i) => (
                <div key={i} className={`bubble ${m.role}`}>
                  <div>{m.text || (busy && m.role === "assistant" ? "Thinking…" : "")}</div>
                  {m.proposals?.map((p) => {
                    const approveLabel =
                      p.target_file === "COACH_PROFILE.md" && firstUseStage === "profile_approval"
                        ? "Approve Initial Profile"
                        : p.target_file === "TRAINING_PLAN.md" && firstUseStage === "plan_approval"
                        ? "Approve Training Plan"
                        : "Approve & Apply";
                    return (
                      <div key={p.id} className="proposal-card">
                        <div className="proposal-header">
                          <div>
                            <div className="proposal-title">{p.target_file}</div>
                            <div className="muted" style={{ fontSize: 12.5, marginTop: 2 }}>
                              {p.reason}
                            </div>
                          </div>
                          <span className={`pill ${p.status === "applied" ? "green" : p.status === "dismissed" ? "" : "accent"}`}>
                            {p.status === "pending_user_approval" ? "Pending Approval" : p.status}
                          </span>
                        </div>
                        {p.expires_on && (
                          <div className="muted" style={{ marginBottom: 8 }}>
                            Expires: {p.expires_on}
                          </div>
                        )}
                        <MarkdownPreview markdown={p.updated_section_markdown || p.diff_preview} />
                        <details className="raw-diff-details">
                          <summary>View Raw Diff</summary>
                          <pre className="raw-diff-pre">{p.diff_preview}</pre>
                        </details>
                        {p.status === "pending_user_approval" && (
                          <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
                            <button
                              className="primary"
                              onClick={() => void handleProposalDecision(i, p.id, true)}
                            >
                              {approveLabel}
                            </button>
                            <button
                              className="secondary"
                              onClick={() => void handleProposalDecision(i, p.id, false)}
                            >
                              Dismiss
                            </button>
                          </div>
                        )}
                      </div>
                    );
                  })}
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
              <h3>AI &amp; Models</h3>
              <p className="muted" style={{ marginTop: 0, marginBottom: 12 }}>
                Default model &amp; thinking level for background takes and new Coach sessions (override anytime from the Coach pill).
              </p>
              <div className="form-row">
                <div>
                  <label className="muted">Global Model</label>
                  <select
                    value={modelSettings.global_model_id}
                    onChange={async (e) => {
                      await invoke("save_global_model_settings", {
                        modelId: e.target.value,
                        thinkingLevel: modelSettings.global_thinking_level,
                      });
                      await reload();
                    }}
                  >
                    {GEMINI_MODEL_OPTIONS.map((opt) => (
                      <option key={opt.id} value={opt.id}>
                        {opt.label}
                      </option>
                    ))}
                  </select>
                </div>
                <div>
                  <label className="muted">Global Thinking Level</label>
                  <select
                    value={modelSettings.global_thinking_level}
                    onChange={async (e) => {
                      await invoke("save_global_model_settings", {
                        modelId: modelSettings.global_model_id,
                        thinkingLevel: e.target.value,
                      });
                      await reload();
                    }}
                  >
                    <option value="LOW">Low</option>
                    <option value="MEDIUM">Medium</option>
                    <option value="HIGH">High</option>
                  </select>
                </div>
              </div>
              <div className="muted" style={{ marginBottom: 8, marginTop: 6 }}>
                Gemini API Key (Windows Credential Manager):{" "}
                <strong>{state.has_gemini_key ? "Saved" : "Not set (Clipboard Packet mode)"}</strong>
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
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 10 }}>
                <h3 style={{ margin: 0 }}>Athlete Profile</h3>
                <button className="secondary" onClick={() => setShowOnboarding(true)}>
                  Edit Athlete Profile / Re-run Onboarding
                </button>
              </div>
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
                    <option value="hybrid_hyrox">Hyrox / Hybrid Athlete</option>
                  </select>
                </div>
              </div>
              <div className="form-row">
                <div>
                  <label className="muted">Goal Time (optional)</label>
                  <input
                    placeholder="Optional — leave blank if goal is to finish"
                    value={draftConfig.objective.targetTimeDescription ?? ""}
                    onChange={(e) =>
                      setDraftConfig({
                        ...draftConfig,
                        objective: {
                          ...draftConfig.objective,
                          targetTimeDescription: e.target.value.trim() ? e.target.value : null,
                        },
                      })
                    }
                  />
                </div>
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
              </div>
              {draftConfig.objective.goalType === "hybrid_hyrox" && (
                <div className="form-row" style={{ marginTop: 10 }}>
                  <div>
                    <label className="muted">Hyrox Division</label>
                    <input
                      placeholder="e.g. Open Men, Pro Women, Doubles"
                      value={draftConfig.objective.hybridProfile?.division ?? ""}
                      onChange={(e) =>
                        setDraftConfig({
                          ...draftConfig,
                          objective: {
                            ...draftConfig.objective,
                            hybridProfile: {
                              division: e.target.value || null,
                              focusStations: draftConfig.objective.hybridProfile?.focusStations ?? [],
                              weeklyHybridSessionsTarget:
                                draftConfig.objective.hybridProfile?.weeklyHybridSessionsTarget ?? 3,
                            },
                          },
                        })
                      }
                    />
                  </div>
                  <div>
                    <label className="muted">Focus Stations (comma-separated)</label>
                    <input
                      placeholder="e.g. Sled Push, Wall Balls, SkiErg"
                      value={(draftConfig.objective.hybridProfile?.focusStations ?? []).join(", ")}
                      onChange={(e) =>
                        setDraftConfig({
                          ...draftConfig,
                          objective: {
                            ...draftConfig.objective,
                            hybridProfile: {
                              division: draftConfig.objective.hybridProfile?.division ?? null,
                              focusStations: e.target.value
                                .split(",")
                                .map((s) => s.trim())
                                .filter(Boolean),
                              weeklyHybridSessionsTarget:
                                draftConfig.objective.hybridProfile?.weeklyHybridSessionsTarget ?? 3,
                            },
                          },
                        })
                      }
                    />
                  </div>
                  <div>
                    <label className="muted">Hybrid Sessions / Week</label>
                    <input
                      type="number"
                      value={draftConfig.objective.hybridProfile?.weeklyHybridSessionsTarget ?? ""}
                      onChange={(e) =>
                        setDraftConfig({
                          ...draftConfig,
                          objective: {
                            ...draftConfig.objective,
                            hybridProfile: {
                              division: draftConfig.objective.hybridProfile?.division ?? null,
                              focusStations: draftConfig.objective.hybridProfile?.focusStations ?? [],
                              weeklyHybridSessionsTarget: e.target.value ? Number(e.target.value) : null,
                            },
                          },
                        })
                      }
                    />
                  </div>
                </div>
              )}

              <div style={{ borderTop: "1px solid var(--border)", paddingTop: 12, marginTop: 12 }}>
                <h4 style={{ margin: "0 0 8px" }}>Advanced · Pace &amp; HR Guardrails</h4>
                <div className="form-row">
                  <div>
                    <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13, marginBottom: 6 }}>
                      <input
                        type="checkbox"
                        style={{ width: "auto" }}
                        checked={draftConfig.medical.enforceEasyHRCeiling}
                        onChange={(e) =>
                          setDraftConfig({
                            ...draftConfig,
                            medical: { ...draftConfig.medical, enforceEasyHRCeiling: e.target.checked },
                          })
                        }
                      />
                      HR-First Governor (enforce Easy HR ceiling)
                    </label>
                    <input
                      type="number"
                      placeholder="Easy HR Ceiling (bpm)"
                      value={draftConfig.medical.easyHRCeilingBPM ?? ""}
                      onChange={(e) =>
                        setDraftConfig({
                          ...draftConfig,
                          medical: {
                            ...draftConfig.medical,
                            easyHRCeilingBPM: e.target.value ? Number(e.target.value) : null,
                          },
                        })
                      }
                    />
                  </div>
                  <div>
                    <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13, marginBottom: 6 }}>
                      <input
                        type="checkbox"
                        style={{ width: "auto" }}
                        checked={draftConfig.medical.enforcePaceFloor}
                        onChange={(e) =>
                          setDraftConfig({
                            ...draftConfig,
                            medical: { ...draftConfig.medical, enforcePaceFloor: e.target.checked },
                          })
                        }
                      />
                      Hard Pace Floor (mm:ss/km)
                    </label>
                    <input
                      placeholder="e.g. 6:30"
                      value={paceFloorInput}
                      onChange={(e) => setPaceFloorInput(e.target.value)}
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
              </div>

              <button
                className="primary"
                onClick={async () => {
                  const parts = paceFloorInput.trim().split(":");
                  let parsedPaceSec: number | null = draftConfig.medical.minPaceSecPerKm;
                  if (!paceFloorInput.trim()) {
                    parsedPaceSec = null;
                  } else if (parts.length === 2) {
                    const m = Number(parts[0]);
                    const s = Number(parts[1]);
                    if (Number.isFinite(m) && Number.isFinite(s) && m >= 0 && s >= 0 && s < 60) {
                      parsedPaceSec = Math.round(m * 60 + s);
                    }
                  }
                  const updatedConfig: AthleteConfig = {
                    ...draftConfig,
                    medical: {
                      ...draftConfig.medical,
                      minPaceSecPerKm: parsedPaceSec,
                    },
                  };
                  await invoke("save_athlete_config", { config: updatedConfig });
                  if (state.user_profile) {
                    await invoke("save_user_profile", {
                      profile: {
                        ...state.user_profile,
                        runner_config: {
                          ...state.user_profile.runner_config,
                          hr_first_governor: updatedConfig.medical.enforceEasyHRCeiling,
                          pace_floor_sec_per_km: parsedPaceSec,
                        },
                      },
                    });
                  }
                  setStatusMsg("Profile & guardrails saved.");
                  await reload();
                }}
              >
                Save Profile
              </button>
              {statusMsg && <span className="muted" style={{ marginLeft: 12 }}>{statusMsg}</span>}
            </div>

            <div className="card">
              <div className="card-header">
                <div>
                  <h3>App Updates</h3>
                  <div className="muted">Installed version: v{appVersion}</div>
                </div>
                <button
                  className="secondary"
                  style={{ display: "flex", alignItems: "center", gap: 6 }}
                  disabled={updateChecking || updateInstalling}
                  onClick={() => void runUpdateCheck(false)}
                >
                  <RefreshCw size={14} />
                  {updateChecking ? "Checking…" : "Check for Updates"}
                </button>
              </div>
              {updateStatus && (
                <div className="muted" style={{ marginBottom: 10 }}>
                  Status: <strong>{updateStatus}</strong>
                </div>
              )}
              {availableUpdate && (
                <div
                  style={{
                    padding: 12,
                    borderRadius: 8,
                    border: "1px solid var(--border)",
                    background: "var(--bg)",
                  }}
                >
                  <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12 }}>
                    <div>
                      <strong>Splits Coach v{availableUpdate.version}</strong>
                      {availableUpdate.body && (
                        <div className="muted" style={{ fontSize: 12, marginTop: 4, whiteSpace: "pre-wrap" }}>
                          {availableUpdate.body}
                        </div>
                      )}
                    </div>
                    {updateReadyToRestart ? (
                      <button className="primary" onClick={() => void relaunch()}>
                        Restart Now
                      </button>
                    ) : (
                      <button
                        className="primary"
                        style={{ display: "flex", alignItems: "center", gap: 6, flexShrink: 0 }}
                        disabled={updateInstalling}
                        onClick={() => void handleInstallUpdate()}
                      >
                        <Download size={14} />
                        {updateInstalling ? "Installing…" : "Download & Install"}
                      </button>
                    )}
                  </div>
                  {updateProgress && (
                    <div style={{ marginTop: 10 }}>
                      <div
                        style={{
                          height: 8,
                          borderRadius: 999,
                          background: "var(--border)",
                          overflow: "hidden",
                        }}
                      >
                        <div
                          style={{
                            height: "100%",
                            width: `${
                              updateProgress.total && updateProgress.total > 0
                                ? Math.min(100, Math.round((updateProgress.downloaded / updateProgress.total) * 100))
                                : 100
                            }%`,
                            background: "var(--accent)",
                            transition: "width 120ms ease",
                          }}
                        />
                      </div>
                      <div className="muted" style={{ fontSize: 12, marginTop: 4 }}>
                        {(updateProgress.downloaded / (1024 * 1024)).toFixed(2)} MB
                        {updateProgress.total
                          ? ` / ${(updateProgress.total / (1024 * 1024)).toFixed(2)} MB (${Math.min(
                              100,
                              Math.round((updateProgress.downloaded / updateProgress.total) * 100)
                            )}%)`
                          : ""}
                      </div>
                    </div>
                  )}
                </div>
              )}
            </div>
          </>
        )}
      </main>
    </div>
  );
}

function WeeklyVolumeChart({ bars }: { bars: { label: string; km: number; runs: number }[] }) {
  const maxKm = Math.max(5, ...bars.map((b) => b.km));
  return (
    <svg viewBox="0 0 560 120" style={{ width: "100%", height: 120 }}>
      {bars.map((b, idx) => {
        const barW = 48;
        const x = 16 + idx * 78;
        const h = Math.round((b.km / maxKm) * 72);
        const y = 86 - h;
        return (
          <g key={idx}>
            <rect x={x} y={y} width={barW} height={Math.max(2, h)} rx={6} fill="var(--accent)" opacity={0.85} />
            <text x={x + barW / 2} y={y - 5} textAnchor="middle" fontSize="10" fill="var(--muted)">
              {b.km > 0 ? b.km.toFixed(1) : ""}
            </text>
            <text x={x + barW / 2} y={106} textAnchor="middle" fontSize="11" fill="var(--muted)">
              {b.label}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

function sanitizeFilenamePart(raw: string): string {
  const cleaned = raw.trim().replace(/[^a-zA-Z0-9._-]+/g, "_").replace(/^_+|_+$/g, "");
  return cleaned || "NA";
}

function parseOptionalNum(raw: string): number | null {
  const n = Number(raw);
  return raw.trim() !== "" && Number.isFinite(n) ? n : null;
}

function parseRatingOrZero(raw: string): number {
  const n = Number(raw);
  if (raw.trim() === "" || !Number.isFinite(n)) return 0;
  return Math.max(0, Math.min(10, Math.round(n)));
}

function ActivityDetailView({
  detail,
  onBack,
  onAnalyze,
  onToggleHyrox,
  onActivityUpdated,
}: {
  detail: ActivityDetailPayload;
  onBack: () => void;
  onAnalyze: () => void;
  onToggleHyrox: (isHyrox: boolean) => Promise<void>;
  onActivityUpdated: (updated: ActivityRecord) => Promise<void>;
}) {
  const a = detail.activity;
  const track = detail.track;
  const forensic = track?.forensic_telemetry ?? null;
  const km = a.distance_m / 1000;
  const durSec = a.moving_duration_sec > 0 ? a.moving_duration_sec : a.duration_sec;
  const avgPace = km > 0.1 && a.moving_duration_sec > 0 ? a.moving_duration_sec / km : null;
  const isHyroxActive = Boolean(a.is_hyrox ?? track?.hyrox_dissection?.is_hyrox_sim);
  const [halfSim, setHalfSim] = useState(false);

  const [shoeModel, setShoeModel] = useState(
    a.post_run_context?.shoe_model || a.shoe_name || ""
  );
  const [shoeSize, setShoeSize] = useState(a.post_run_context?.shoe_size ?? "");
  const [preWeight, setPreWeight] = useState(
    a.post_run_context?.pre_run_weight_kg != null ? String(a.post_run_context.pre_run_weight_kg) : ""
  );
  const [postWeight, setPostWeight] = useState(
    a.post_run_context?.post_run_weight_kg != null ? String(a.post_run_context.post_run_weight_kg) : ""
  );
  const [fluidMl, setFluidMl] = useState(
    a.post_run_context?.fluid_consumed_ml != null ? String(a.post_run_context.fluid_consumed_ml) : ""
  );
  const [fluidIntakeConfirmed, setFluidIntakeConfirmed] = useState(
    Boolean(a.post_run_context?.fluid_intake_confirmed)
  );
  const [tempeChoice, setTempeChoice] = useState<"unconfirmed" | "yes" | "no">(
    a.post_run_context?.tempe_sensor_used === true
      ? "yes"
      : a.post_run_context?.tempe_sensor_used === false
        ? "no"
        : "unconfirmed"
  );
  const [initialPreWeight, setInitialPreWeight] = useState(
    a.post_run_context?.pre_run_weight_kg != null ? String(a.post_run_context.pre_run_weight_kg) : ""
  );
  const [initialPostWeight, setInitialPostWeight] = useState(
    a.post_run_context?.post_run_weight_kg != null ? String(a.post_run_context.post_run_weight_kg) : ""
  );
  const [leftAchilles, setLeftAchilles] = useState(
    String(a.post_run_context?.orthopedic?.left_achilles ?? 0)
  );
  const [lumbarSciatica, setLumbarSciatica] = useState(
    String(a.post_run_context?.orthopedic?.lumbar_sciatica ?? 0)
  );
  const [rightAnkle, setRightAnkle] = useState(
    String(a.post_run_context?.orthopedic?.right_ankle ?? 0)
  );
  const [calvesSoleus, setCalvesSoleus] = useState(
    String(a.post_run_context?.orthopedic?.calves_soleus ?? 0)
  );
  const [savingContext, setSavingContext] = useState(false);
  const [refreshingMeta, setRefreshingMeta] = useState(false);
  const [contextNotice, setContextNotice] = useState<string | null>(null);
  const [metaNotice, setMetaNotice] = useState<string | null>(null);
  const [csvNotice, setCsvNotice] = useState<string | null>(null);

  useEffect(() => {
    const ctx = a.post_run_context;
    const preStr = ctx?.pre_run_weight_kg != null ? String(ctx.pre_run_weight_kg) : "";
    const postStr = ctx?.post_run_weight_kg != null ? String(ctx.post_run_weight_kg) : "";
    setShoeModel(ctx?.shoe_model || a.shoe_name || "");
    setShoeSize(ctx?.shoe_size ?? "");
    setPreWeight(preStr);
    setPostWeight(postStr);
    setInitialPreWeight(preStr);
    setInitialPostWeight(postStr);
    setFluidMl(ctx?.fluid_consumed_ml != null ? String(ctx.fluid_consumed_ml) : "");
    setFluidIntakeConfirmed(Boolean(ctx?.fluid_intake_confirmed));
    setTempeChoice(
      ctx?.tempe_sensor_used === true ? "yes" : ctx?.tempe_sensor_used === false ? "no" : "unconfirmed"
    );
    setLeftAchilles(String(ctx?.orthopedic?.left_achilles ?? 0));
    setLumbarSciatica(String(ctx?.orthopedic?.lumbar_sciatica ?? 0));
    setRightAnkle(String(ctx?.orthopedic?.right_ankle ?? 0));
    setCalvesSoleus(String(ctx?.orthopedic?.calves_soleus ?? 0));
    setContextNotice(null);
    setMetaNotice(null);
    setCsvNotice(null);
  }, [a.garmin_id]);

  async function handleRefreshMetadata() {
    setRefreshingMeta(true);
    setMetaNotice(null);
    try {
      const updated = await invoke<ActivityRecord>("refresh_activity_metadata", {
        activityId: a.garmin_id,
      });
      await onActivityUpdated(updated);
      setMetaNotice("Metadata refreshed from Garmin Connect");
    } catch (err) {
      setMetaNotice(`Refresh failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setRefreshingMeta(false);
    }
  }

  const preNum = parseOptionalNum(preWeight);
  const postNum = parseOptionalNum(postWeight);
  const fluidNum = parseOptionalNum(fluidMl) ?? 0;
  const liveProvisionalLossKg =
    preNum != null && postNum != null && preNum > 0 && postNum > 0
      ? preNum - postNum
      : null;
  const liveFinalLossKg =
    fluidIntakeConfirmed && liveProvisionalLossKg != null
      ? liveProvisionalLossKg + fluidNum / 1000
      : null;
  const durHours = durSec / 3600;
  const liveProvisionalSweatRateLHr =
    liveProvisionalLossKg != null && durSec > 60 ? liveProvisionalLossKg / durHours : null;
  const liveFinalSweatRateLHr =
    liveFinalLossKg != null && durSec > 60 ? liveFinalLossKg / durHours : null;

  const baseTempSource = track?.temp_source ?? forensic?.temp_source ?? a.fit_temp_source ?? "watch_internal";
  const effectiveTempSource =
    baseTempSource === "garmin_tempe_fit"
      ? "garmin_tempe_fit"
      : tempeChoice === "yes"
        ? "user_confirmed_tempe"
        : "watch_internal";

  async function handleExport1HzCsv() {
    const datePart = a.start_iso.slice(0, 10);
    const namePart = sanitizeFilenamePart(a.name || a.type_key);
    const distPart = `${km.toFixed(2)}km`;
    const bpmPart = a.average_hr ? `${Math.round(a.average_hr)}bpm` : "NoHR";
    const shoeLabel = [shoeModel.trim(), shoeSize.trim()].filter(Boolean).join("_");
    const shoePart = sanitizeFilenamePart(shoeLabel || a.shoe_name || "NoShoe");
    const defaultName = `${datePart}_${namePart}_${distPart}_${bpmPart}_${shoePart}.csv`;
    const targetPath = await save({
      defaultPath: defaultName,
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    if (!targetPath) return;
    try {
      const count = await invoke<number>("export_1hz_csv", {
        activityId: a.garmin_id,
        targetPath,
      });
      setCsvNotice(`Exported ${count} 1Hz rows`);
    } catch (err) {
      setCsvNotice(`CSV error: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  async function handleSavePostRunContext() {
    setSavingContext(true);
    setContextNotice(null);
    try {
      const preEdited = Boolean(a.post_run_context?.pre_weight_manually_edited) || preWeight !== initialPreWeight;
      const postEdited = Boolean(a.post_run_context?.post_weight_manually_edited) || postWeight !== initialPostWeight;
      const existingWs = a.post_run_context?.weight_source ?? null;
      const nextWs = preEdited ? "manual" : existingWs ?? (preNum != null ? "manual" : null);
      const tempeUsed = tempeChoice === "yes" ? true : tempeChoice === "no" ? false : null;
      const payload: PostRunContext = {
        shoe_model: shoeModel.trim(),
        shoe_size: shoeSize.trim(),
        pre_run_weight_kg: preNum,
        post_run_weight_kg: postNum,
        fluid_consumed_ml: parseOptionalNum(fluidMl),
        net_fluid_deficit_kg: null,
        net_sweat_rate_l_hr: null,
        pre_weight_manually_edited: preEdited,
        post_weight_manually_edited: postEdited,
        fluid_intake_confirmed: fluidIntakeConfirmed,
        weight_source: nextWs,
        tempe_sensor_used: tempeUsed,
        orthopedic: {
          left_achilles: parseRatingOrZero(leftAchilles),
          lumbar_sciatica: parseRatingOrZero(lumbarSciatica),
          right_ankle: parseRatingOrZero(rightAnkle),
          calves_soleus: parseRatingOrZero(calvesSoleus),
        },
      };
      const updated = await invoke<ActivityRecord>("save_activity_post_run_context", {
        activityId: a.garmin_id,
        context: payload,
      });
      await onActivityUpdated(updated);
      setContextNotice("Saved post-run context & sweat metrics");
    } catch (err) {
      setContextNotice(`Save error: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setSavingContext(false);
    }
  }

  const HYROX_STATION_LABELS = [
    "1000m SkiErg",
    "50m Sled Push",
    "50m Sled Pull",
    "80m Burpee Broad Jumps",
    "1000m Row",
    "200m Farmers Carry",
    "100m Sandbag Lunges",
    "100 Wall Balls",
  ];
  const roundCount = halfSim ? 4 : 8;

  return (
    <>
      <div className="card">
        <div className="card-header">
          <button className="secondary" style={{ display: "flex", alignItems: "center", gap: 6 }} onClick={onBack}>
            <ArrowLeft size={14} /> Back
          </button>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
            {csvNotice && <span className="pill">{csvNotice}</span>}
            {metaNotice && <span className="pill green">{metaNotice}</span>}
            <button
              className="secondary"
              style={{ display: "flex", alignItems: "center", gap: 6 }}
              disabled={refreshingMeta}
              onClick={() => void handleRefreshMetadata()}
            >
              <RefreshCw size={14} /> {refreshingMeta ? "Refreshing…" : "Refresh Metadata"}
            </button>
            <button
              className="secondary"
              style={{ display: "flex", alignItems: "center", gap: 6 }}
              onClick={() => void handleExport1HzCsv()}
            >
              <Download size={14} /> Export 1Hz CSV
            </button>
            <button
              className="secondary"
              onClick={() => void onToggleHyrox(!isHyroxActive)}
            >
              {isHyroxActive ? "HYROX / Hybrid Sim ✓" : "Mark as HYROX / Hybrid Sim"}
            </button>
            <button className="primary" style={{ display: "flex", alignItems: "center", gap: 6 }} onClick={onAnalyze}>
              <Sparkles size={14} /> Analyze in Coach
            </button>
          </div>
        </div>
        <h2 style={{ margin: "4px 0" }}>{a.name || a.type_key}</h2>
        <div className="muted" style={{ marginBottom: 14 }}>
          {a.start_iso.slice(0, 10)} · {a.type_key}
        </div>
        {a.garmin_notes && a.garmin_notes.trim().length > 0 && (
          <div
            style={{
              marginBottom: 14,
              padding: "10px 12px",
              borderRadius: 8,
              border: "1px solid var(--accent-border)",
              background: "var(--accent-soft-bg)",
            }}
          >
            <div style={{ fontSize: 12, fontWeight: 700, color: "var(--accent-primary)", marginBottom: 4 }}>
              Garmin Connect Notes
            </div>
            <div style={{ fontSize: 13.5, lineHeight: 1.5, whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
              {a.garmin_notes.trim()}
            </div>
          </div>
        )}
        {track?.hyrox_dissection && (
          <div
            style={{
              marginBottom: 14,
              padding: 12,
              borderRadius: 8,
              border: "1px solid var(--border)",
              background: "var(--bg)",
            }}
          >
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
              <strong>HYROX FIT Lap Dissector</strong>
              <span className="pill green">
                {track.hyrox_dissection.run_splits.length} runs · {track.hyrox_dissection.station_splits.length} stations
                {track.hyrox_dissection.roxzone_transitions.length > 0
                  ? ` · ${track.hyrox_dissection.roxzone_transitions.length} roxzone`
                  : ""}
              </span>
            </div>
            <div className="muted" style={{ fontSize: 13 }}>
              Clean Run Pace: <strong>{formatPace(track.hyrox_dissection.clean_run_pace_sec_per_km)}</strong>
              {" · "}Clean Aerobic Efficiency:{" "}
              <strong>{track.hyrox_dissection.clean_aerobic_efficiency?.toFixed(3) ?? "–"}</strong>
              {" · "}Compromised Decay:{" "}
              <strong>
                {track.hyrox_dissection.compromised_pace_decay_sec != null
                  ? `${track.hyrox_dissection.compromised_pace_decay_sec > 0 ? "+" : ""}${track.hyrox_dissection.compromised_pace_decay_sec.toFixed(1)} s/km`
                  : "–"}
              </strong>
            </div>
          </div>
        )}
        {isHyroxActive && track?.hyrox_dissection && (
          <div style={{ marginBottom: 14 }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
              <strong>Dual-Column {halfSim ? "4×4 Half-Sim" : "8×8"} HYROX Round Matrix</strong>
              <button className="secondary" onClick={() => setHalfSim((v) => !v)}>
                {halfSim ? "Switch to 8×8 Full" : "4×4 Half-Sim"}
              </button>
            </div>
            <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8 }}>
              {Array.from({ length: roundCount }, (_, idx) => {
                const run = track.hyrox_dissection!.run_splits[idx];
                const station = track.hyrox_dissection!.station_splits[idx];
                const rox = track.hyrox_dissection!.roxzone_transitions[idx];
                return (
                  <div
                    key={idx}
                    style={{
                      padding: 10,
                      borderRadius: 8,
                      border: "1px solid var(--border)",
                      background: "var(--bg)",
                      fontSize: 12,
                    }}
                  >
                    <div style={{ fontWeight: 600, marginBottom: 4 }}>
                      Round {idx + 1} · {HYROX_STATION_LABELS[idx] ?? `Station ${idx + 1}`}
                    </div>
                    <div style={{ display: "flex", justifyContent: "space-between" }}>
                      <span>
                        Run {idx + 1}:{" "}
                        <strong>
                          {run ? `${formatDuration(run.moving_sec)} (${formatPace(run.pace_sec_per_km)})` : "–"}
                        </strong>
                      </span>
                      <span className="muted">{run?.avg_hr_bpm ? `${Math.round(run.avg_hr_bpm)} bpm` : ""}</span>
                    </div>
                    <div style={{ display: "flex", justifyContent: "space-between", marginTop: 2 }}>
                      <span>
                        Station: <strong>{station ? formatDuration(station.moving_sec) : "–"}</strong>
                      </span>
                      <span className="muted">
                        {rox ? `Roxzone ${formatDuration(rox.moving_sec)}` : ""}
                      </span>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}
        <div className="grid">
          <div className="card">
            <div className="muted">Distance</div>
            <div className="metric-val">{km.toFixed(2)} km</div>
          </div>
          <div className="card">
            <div className="muted">Duration</div>
            <div className="metric-val">{formatDuration(a.moving_duration_sec || a.duration_sec)}</div>
          </div>
          <div className="card">
            <div className="muted">Avg Pace</div>
            <div className="metric-val">{formatPace(avgPace)}</div>
          </div>
          <div className="card">
            <div className="muted">Avg / Max HR</div>
            <div className="metric-val">
              {a.average_hr ? Math.round(a.average_hr) : "–"} / {a.max_hr ? Math.round(a.max_hr) : "–"}
            </div>
          </div>
          <div className="card">
            <div className="muted">Cadence / Elev</div>
            <div className="metric-val">
              {a.cadence_spm ? Math.round(a.cadence_spm) : "–"} · {Math.round(a.elevation_gain_m)}m
            </div>
          </div>
          <div className="card">
            <div className="muted">Cardiac Decoupling</div>
            <div className="metric-val">
              {track?.decoupling_pct != null ? `${track.decoupling_pct.toFixed(1)}%` : "–"}
            </div>
            {track && (
              <span className={`pill ${track.audit.compliant ? "green" : "amber"}`}>
                {track.audit.compliant ? "Guardrails Compliant" : "Guardrail Violation"}
              </span>
            )}
          </div>
        </div>
      </div>

      {track && track.chart_points.length > 1 && (
        <div className="card">
          <h3>Heart Rate (bpm) & Pace across Distance</h3>
          <FitDualChart points={track.chart_points} />
        </div>
      )}

      {track && track.splits.length > 0 && (
        <div className="card">
          <h3>Per-km Splits</h3>
          <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13 }}>
            <thead>
              <tr style={{ textAlign: "left", borderBottom: "1px solid var(--border)" }}>
                <th style={{ padding: 6 }}>Km</th>
                <th style={{ padding: 6 }}>Pace</th>
                <th style={{ padding: 6 }}>Avg HR</th>
                <th style={{ padding: 6 }}>Cadence</th>
                <th style={{ padding: 6 }}>Elev</th>
              </tr>
            </thead>
            <tbody>
              {track.splits.map((s) => (
                <tr key={s.km_index} style={{ borderBottom: "1px solid var(--border)" }}>
                  <td style={{ padding: 6 }}>{s.km_index}</td>
                  <td style={{ padding: 6 }}>{formatPace(s.pace_sec_per_km)}</td>
                  <td style={{ padding: 6 }}>{s.avg_hr_bpm ? `${Math.round(s.avg_hr_bpm)} bpm` : "–"}</td>
                  <td style={{ padding: 6 }}>{s.avg_cadence_spm ? `${Math.round(s.avg_cadence_spm)} spm` : "–"}</td>
                  <td style={{ padding: 6 }}>+{s.elevation_gain_m.toFixed(0)} m</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {forensic && (
        <div className="card">
          <div className="card-header">
            <h3 style={{ margin: 0 }}>1Hz Forensic Telemetry ({forensic.sample_count} samples)</h3>
            <span className="pill">
              Walk {Math.round(forensic.walk_seconds)}s ({forensic.walk_segments}) · Run {Math.round(forensic.run_seconds)}s ({forensic.run_segments})
            </span>
          </div>
          <div className="grid" style={{ marginBottom: 14 }}>
            <div className="card">
              <div className="muted">Pw:Hr Decoupling</div>
              <div className="metric-val">
                {forensic.pw_hr_decoupling_pct != null ? `${forensic.pw_hr_decoupling_pct.toFixed(2)}%` : "–"}
              </div>
            </div>
            <div className="card">
              <div className="muted">Pa:Hr Decoupling</div>
              <div className="metric-val">
                {forensic.pa_hr_decoupling_pct != null ? `${forensic.pa_hr_decoupling_pct.toFixed(2)}%` : "–"}
              </div>
            </div>
            <div className="card">
              <div className="muted">Left Stance &gt; 52.8%</div>
              <div
                className="metric-val"
                style={{
                  color:
                    forensic.left_stance_over_52_8_pct != null && forensic.left_stance_over_52_8_pct > 15
                      ? "var(--red)"
                      : forensic.left_stance_over_52_8_pct != null && forensic.left_stance_over_52_8_pct > 0
                      ? "var(--amber)"
                      : undefined,
                }}
              >
                {forensic.left_stance_over_52_8_pct != null ? `${forensic.left_stance_over_52_8_pct.toFixed(1)}%` : "–"}
              </div>
            </div>
            <div className="card">
              <div className="muted">Respiration (brpm)</div>
              <div className="metric-val">
                {forensic.respiration_brpm ? `${forensic.respiration_brpm.mean.toFixed(1)} (${forensic.respiration_brpm.max.toFixed(0)})` : "–"}
              </div>
            </div>
            <div className="card">
              <div className="muted">Impact Load</div>
              <div className="metric-val">
                {forensic.impact_load_total != null ? forensic.impact_load_total.toFixed(1) : "–"}
              </div>
            </div>
            <div className="card">
              <div className="muted">Zone Breaches (VT1 / VT2 / Z5)</div>
              <div
                className="metric-val"
                style={{
                  fontSize: 16,
                  color:
                    forensic.z5_breach_seconds > 0
                      ? "var(--red)"
                      : forensic.vt1_breach_seconds > 0
                      ? "var(--amber)"
                      : undefined,
                }}
              >
                {Math.round(forensic.vt1_breach_seconds)}s / {Math.round(forensic.vt2_breach_seconds)}s / {Math.round(forensic.z5_breach_seconds)}s
              </div>
            </div>
          </div>

          {forensic.directional_vectors && (
            <div style={{ marginBottom: 14, padding: 12, borderRadius: 8, border: "1px solid var(--border)", background: "var(--bg)" }}>
              <strong>
                Outbound vs Inbound Directional Split (turnaround {(forensic.directional_vectors.turnaround_distance_m / 1000).toFixed(2)} km ·{" "}
                {Math.round(forensic.directional_vectors.max_displacement_m)}m max displacement)
              </strong>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 10, marginTop: 8, fontSize: 13 }}>
                <div>
                  <div className="muted">Outbound ({forensic.directional_vectors.outbound.sample_count}s)</div>
                  <div>
                    HR {forensic.directional_vectors.outbound.mean_hr?.toFixed(0) ?? "–"} bpm · Pace{" "}
                    {formatPace(forensic.directional_vectors.outbound.mean_pace_sec_per_km)} ·{" "}
                    {forensic.directional_vectors.outbound.mean_power_w?.toFixed(0) ?? "–"} W · GCT{" "}
                    {forensic.directional_vectors.outbound.mean_gct_ms?.toFixed(0) ?? "–"} ms
                  </div>
                </div>
                <div>
                  <div className="muted">Inbound ({forensic.directional_vectors.inbound.sample_count}s)</div>
                  <div>
                    HR {forensic.directional_vectors.inbound.mean_hr?.toFixed(0) ?? "–"} bpm · Pace{" "}
                    {formatPace(forensic.directional_vectors.inbound.mean_pace_sec_per_km)} ·{" "}
                    {forensic.directional_vectors.inbound.mean_power_w?.toFixed(0) ?? "–"} W · GCT{" "}
                    {forensic.directional_vectors.inbound.mean_gct_ms?.toFixed(0) ?? "–"} ms
                  </div>
                </div>
              </div>
            </div>
          )}

          {forensic.velocity_bands.length > 0 && (
            <div style={{ marginBottom: 14 }}>
              <strong>Velocity-Band Biomechanics (Active Running)</strong>
              <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 12, marginTop: 6 }}>
                <thead>
                  <tr style={{ textAlign: "left", borderBottom: "1px solid var(--border)" }}>
                    <th style={{ padding: 6 }}>Band</th>
                    <th style={{ padding: 6 }}>Sec</th>
                    <th style={{ padding: 6 }}>GCT (ms)</th>
                    <th style={{ padding: 6 }}>Cadence</th>
                    <th style={{ padding: 6 }}>Vert Osc</th>
                    <th style={{ padding: 6 }}>Vert Ratio</th>
                    <th style={{ padding: 6 }}>Stride (mm)</th>
                    <th style={{ padding: 6 }}>Power (W)</th>
                    <th style={{ padding: 6 }}>L Stance %</th>
                  </tr>
                </thead>
                <tbody>
                  {forensic.velocity_bands.map((vb) => (
                    <tr key={vb.band} style={{ borderBottom: "1px solid var(--border)" }}>
                      <td style={{ padding: 6 }}><strong>{vb.band}</strong></td>
                      <td style={{ padding: 6 }}>{vb.sample_count}</td>
                      <td style={{ padding: 6 }}>{vb.mean_gct_ms?.toFixed(1) ?? "–"}</td>
                      <td style={{ padding: 6 }}>{vb.mean_cadence_spm?.toFixed(0) ?? "–"}</td>
                      <td style={{ padding: 6 }}>{vb.mean_vert_osc_mm?.toFixed(1) ?? "–"}</td>
                      <td style={{ padding: 6 }}>{vb.mean_vert_ratio_pct?.toFixed(1) ?? "–"}</td>
                      <td style={{ padding: 6 }}>{vb.mean_step_length_mm?.toFixed(0) ?? "–"}</td>
                      <td style={{ padding: 6 }}>{vb.mean_power_w?.toFixed(0) ?? "–"}</td>
                      <td style={{ padding: 6 }}>{vb.mean_left_stance_balance_pct?.toFixed(1) ?? "–"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {forensic.micro_events.length > 0 && (
            <div>
              <strong>Detected 1Hz Micro-Events ({forensic.micro_events.length})</strong>
              <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 6, fontSize: 12 }}>
                {forensic.micro_events.slice(0, 12).map((ev, i) => (
                  <div key={i} style={{ padding: "4px 8px", borderRadius: 6, background: "var(--bg)" }}>
                    <strong>[{ev.kind}]</strong> @{Math.round(ev.elapsed_s)}s: {ev.summary}
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      <div className="card">
        <div className="card-header">
          <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
            <h3 style={{ margin: 0 }}>Post-Run Context &amp; Sweat Matrix</h3>
            {a.post_run_context?.weight_source && (
              <span className="pill">weight: {a.post_run_context.weight_source}</span>
            )}
            <span className={`pill ${effectiveTempSource === "watch_internal" ? "amber" : "green"}`}>
              temp: {effectiveTempSource}
            </span>
          </div>
          {contextNotice && <span className="pill">{contextNotice}</span>}
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
          <label>
            <div className="muted" style={{ fontSize: 12 }}>Shoe Model</div>
            <input
              type="text"
              value={shoeModel}
              onChange={(e) => setShoeModel(e.target.value)}
              placeholder="e.g. Superblast 2"
            />
          </label>
          <label>
            <div className="muted" style={{ fontSize: 12 }}>Shoe Size</div>
            <input
              type="text"
              value={shoeSize}
              onChange={(e) => setShoeSize(e.target.value)}
              placeholder="e.g. EU 43"
            />
          </label>
          <label>
            <div className="muted" style={{ fontSize: 12 }}>Fluid Consumed (ml)</div>
            <input
              type="number"
              value={fluidMl}
              onChange={(e) => {
                const v = e.target.value;
                setFluidMl(v);
                if (v.trim() !== "") setFluidIntakeConfirmed(true);
              }}
              placeholder="e.g. 500"
            />
          </label>
          <label>
            <div className="muted" style={{ fontSize: 12 }}>Garmin Tempe Sensor Used</div>
            <select
              value={tempeChoice}
              onChange={(e) => setTempeChoice(e.target.value as "unconfirmed" | "yes" | "no")}
            >
              <option value="unconfirmed">Unconfirmed</option>
              <option value="yes">Yes (Garmin Tempe)</option>
              <option value="no">No (Watch Internal)</option>
            </select>
          </label>
          <label>
            <div className="muted" style={{ fontSize: 12 }}>Pre-Run Weight (kg)</div>
            <input
              type="number"
              step="0.05"
              value={preWeight}
              onChange={(e) => setPreWeight(e.target.value)}
              placeholder="e.g. 74.2"
            />
          </label>
          <label>
            <div className="muted" style={{ fontSize: 12 }}>Post-Run Weight (kg)</div>
            <input
              type="number"
              step="0.05"
              value={postWeight}
              onChange={(e) => setPostWeight(e.target.value)}
              placeholder="e.g. 73.1"
            />
          </label>
        </div>

        <label style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 10 }}>
          <input
            type="checkbox"
            checked={fluidIntakeConfirmed}
            onChange={(e) => setFluidIntakeConfirmed(e.target.checked)}
          />
          <span style={{ fontSize: 13 }}>Confirm Intra-Run Fluid Intake (upgrades Sweat Matrix to Final Verified)</span>
        </label>

        <div className="muted" style={{ fontSize: 13, marginTop: 10, marginBottom: 10 }}>
          Provisional Rate (pre - post):{" "}
          <strong>{liveProvisionalSweatRateLHr != null ? `${liveProvisionalSweatRateLHr.toFixed(2)} L/hr` : "–"}</strong>
          {" · "}Final Verified:{" "}
          <strong>{liveFinalSweatRateLHr != null ? `${liveFinalSweatRateLHr.toFixed(2)} L/hr` : "Pending Intake Confirmation"}</strong>
        </div>

        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr 1fr", gap: 10 }}>
          <label>
            <div className="muted" style={{ fontSize: 12 }}>Left Achilles (0–10)</div>
            <input type="number" min={0} max={10} value={leftAchilles} onChange={(e) => setLeftAchilles(e.target.value)} />
          </label>
          <label>
            <div className="muted" style={{ fontSize: 12 }}>Lumbar / Sciatica (0–10)</div>
            <input type="number" min={0} max={10} value={lumbarSciatica} onChange={(e) => setLumbarSciatica(e.target.value)} />
          </label>
          <label>
            <div className="muted" style={{ fontSize: 12 }}>Right Ankle (0–10)</div>
            <input type="number" min={0} max={10} value={rightAnkle} onChange={(e) => setRightAnkle(e.target.value)} />
          </label>
          <label>
            <div className="muted" style={{ fontSize: 12 }}>Calves / Soleus (0–10)</div>
            <input type="number" min={0} max={10} value={calvesSoleus} onChange={(e) => setCalvesSoleus(e.target.value)} />
          </label>
        </div>

        <div style={{ marginTop: 12, display: "flex", justifyContent: "flex-end" }}>
          <button className="primary" disabled={savingContext} onClick={() => void handleSavePostRunContext()}>
            {savingContext ? "Saving…" : "Save Post-Run Context"}
          </button>
        </div>
      </div>
    </>
  );
}

function FitDualChart({ points }: { points: FitTrackPoint[] }) {
  const maxDist = Math.max(1, points[points.length - 1]?.distance_m ?? 1);
  const hrs = points.map((p) => p.heart_rate_bpm).filter((v): v is number => v != null);
  const minHr = hrs.length ? Math.min(...hrs) - 5 : 90;
  const maxHr = hrs.length ? Math.max(...hrs) + 5 : 190;

  const paces = points.map((p) => p.pace_sec_per_km).filter((v): v is number => v != null && v < 900);
  const minPace = paces.length ? Math.min(...paces) - 10 : 240;
  const maxPace = paces.length ? Math.max(...paces) + 10 : 480;

  const hrPath = points
    .filter((p) => p.heart_rate_bpm != null)
    .map((p, i) => {
      const x = (p.distance_m / maxDist) * 600;
      const y = 140 - (((p.heart_rate_bpm! - minHr) / Math.max(1, maxHr - minHr)) * 120);
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");

  const pacePath = points
    .filter((p) => p.pace_sec_per_km != null && p.pace_sec_per_km < 900)
    .map((p, i) => {
      const x = (p.distance_m / maxDist) * 600;
      const y = 140 - (((maxPace - p.pace_sec_per_km!) / Math.max(1, maxPace - minPace)) * 120);
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");

  return (
    <svg viewBox="0 0 600 155" style={{ width: "100%", height: 160 }}>
      {pacePath && <path d={pacePath} fill="none" stroke="var(--accent)" strokeWidth="2" opacity={0.8} />}
      {hrPath && <path d={hrPath} fill="none" stroke="var(--red)" strokeWidth="2" />}
      <text x={8} y={14} fontSize="11" fill="var(--red)">HR (bpm)</text>
      <text x={80} y={14} fontSize="11" fill="var(--accent)">Pace (/km)</text>
    </svg>
  );
}
