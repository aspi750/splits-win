import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronLeft, ChevronRight, Moon, RefreshCw, Sparkles } from "lucide-react";
import type { DailyBriefingRecord, DayRecord } from "../types";

type SleepRange = 30 | 90 | 365;

interface SleepViewProps {
  days: DayRecord[];
  hasGeminiKey: boolean;
  loading: boolean;
  cachedTake: DailyBriefingRecord | null;
  onTakeUpdated: (take: DailyBriefingRecord) => void;
}

interface ParsedNight {
  date: string;
  asleepSec: number;
  needSec: number | null;
  deepSec: number;
  lightSec: number;
  remSec: number;
  awakeSec: number;
  score: number | null;
  quality: string | null;
  bedtimeHours: number | null;
  wakeHours: number | null;
  midsleepHours: number | null;
  inBedHours: number | null;
  efficiencyPct: number | null;
  isWeekend: boolean;
  respirationAvg: number | null;
  respirationLow: number | null;
  respirationHigh: number | null;
  sleepStress: number | null;
  awakeCount: number | null;
  avgHr: number | null;
}

const OPTIMAL_STAGES = [
  { stage: "Deep", low: 16, high: 33, color: "var(--chart-indigo)" },
  { stage: "Light", low: 30, high: 64, color: "var(--chart-blue)" },
  { stage: "REM", low: 21, high: 31, color: "var(--chart-purple)" },
] as const;

function parseClockRelative(iso: string | null | undefined, wakeDateIso: string): number | null {
  if (!iso) return null;
  const trimmed = iso.trim();
  const normalized = trimmed.slice(0, 19).replace(" ", "T");
  const dt = new Date(normalized);
  if (Number.isNaN(dt.getTime())) return null;
  const [y, m, d] = wakeDateIso.split("-").map(Number);
  const prevMidnight = new Date(y, (m || 1) - 1, (d || 1) - 1, 0, 0, 0);
  return (dt.getTime() - prevMidnight.getTime()) / 3_600_000 - 24;
}

function formatClock(hoursAfterMidnight: number | null): string {
  if (hoursAfterMidnight == null || !Number.isFinite(hoursAfterMidnight)) return "–";
  let mins = Math.round(hoursAfterMidnight * 60) % (24 * 60);
  if (mins < 0) mins += 24 * 60;
  return `${String(Math.floor(mins / 60)).padStart(2, "0")}:${String(mins % 60).padStart(2, "0")}`;
}

function formatHm(seconds: number | null | undefined): string {
  if (!seconds || !Number.isFinite(seconds) || seconds <= 0) return "–";
  const mins = Math.round(seconds / 60);
  if (mins >= 60) return `${Math.floor(mins / 60)} h ${mins % 60} min`;
  return `${mins} min`;
}

function mean(xs: number[]): number | null {
  return xs.length === 0 ? null : xs.reduce((a, b) => a + b, 0) / xs.length;
}

function median(xs: number[]): number | null {
  if (xs.length === 0) return null;
  const s = [...xs].sort((a, b) => a - b);
  const mid = Math.floor(s.length / 2);
  return s.length % 2 === 1 ? s[mid] : (s[mid - 1] + s[mid]) / 2;
}

function stdDev(xs: number[]): number | null {
  if (xs.length < 2) return null;
  const m = mean(xs)!;
  const variance = xs.reduce((s, x) => s + (x - m) ** 2, 0) / (xs.length - 1);
  return Math.sqrt(variance);
}

export function SleepView({
  days,
  hasGeminiKey,
  loading,
  cachedTake,
  onTakeUpdated,
}: SleepViewProps) {
  const [range, setRange] = useState<SleepRange>(30);
  const [nightIndex, setNightIndex] = useState<number | null>(null);
  const [takeBusy, setTakeBusy] = useState(false);
  const [takeError, setTakeError] = useState<string | null>(null);
  const attemptedForRef = useRef<string | null>(null);

  const nights = useMemo<ParsedNight[]>(() => {
    const out: ParsedNight[] = [];
    const sorted = [...days].sort((a, b) => a.date.localeCompare(b.date));
    for (const d of sorted) {
      const asleep = d.sleep_seconds ?? d.sleep_duration_sec ?? 0;
      if (!asleep || asleep <= 0) continue;
      const bed = parseClockRelative(d.sleep_start, d.date);
      const wake = parseClockRelative(d.sleep_end, d.date);
      const validWindow = bed != null && wake != null && wake > bed;
      const inBedHours = validWindow ? wake! - bed! : null;
      const midsleepHours = validWindow ? bed! + (wake! - bed!) / 2 : null;
      const needMin = d.sleep_need_minutes ?? d.sleep_baseline_minutes ?? null;
      const [y, m, day] = d.date.split("-").map(Number);
      const wd = new Date(y, (m || 1) - 1, day || 1).getDay();
      out.push({
        date: d.date,
        asleepSec: asleep,
        needSec: needMin && needMin > 0 ? needMin * 60 : null,
        deepSec: Math.max(0, d.sleep_deep_sec ?? 0),
        lightSec: Math.max(0, d.sleep_light_sec ?? 0),
        remSec: Math.max(0, d.sleep_rem_sec ?? 0),
        awakeSec: Math.max(0, d.sleep_awake_sec ?? 0),
        score: d.sleep_score ?? null,
        quality: d.sleep_quality ?? null,
        bedtimeHours: validWindow ? bed : null,
        wakeHours: validWindow ? wake : null,
        midsleepHours,
        inBedHours,
        efficiencyPct:
          inBedHours && inBedHours > 0
            ? Math.min(1, asleep / (inBedHours * 3600)) * 100
            : null,
        isWeekend: wd === 0 || wd === 6,
        respirationAvg: d.sleep_respiration_avg ?? null,
        respirationLow: d.sleep_respiration_low ?? null,
        respirationHigh: d.sleep_respiration_high ?? null,
        sleepStress: d.sleep_stress ?? null,
        awakeCount: d.sleep_awake_count ?? null,
        avgHr: d.sleep_avg_hr ?? null,
      });
    }
    return out;
  }, [days]);

  const latestNightDate = nights.length > 0 ? nights[nights.length - 1].date : null;

  useEffect(() => {
    if (!hasGeminiKey || !latestNightDate) return;
    if (cachedTake && cachedTake.date === latestNightDate) return;
    if (attemptedForRef.current === latestNightDate) return;
    attemptedForRef.current = latestNightDate;
    let cancelled = false;
    setTakeBusy(true);
    setTakeError(null);
    void invoke<DailyBriefingRecord>("get_or_generate_sleep_take", { forceRefresh: false })
      .then((res) => {
        if (!cancelled) onTakeUpdated(res);
      })
      .catch((err) => {
        if (!cancelled) setTakeError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setTakeBusy(false);
      });
    return () => {
      cancelled = true;
    };
  }, [hasGeminiKey, latestNightDate, cachedTake, onTakeUpdated]);

  const shownNights = useMemo(() => {
    if (nights.length === 0) return [];
    const cutoff = new Date();
    cutoff.setDate(cutoff.getDate() - range);
    const cutoffIso = `${cutoff.getFullYear()}-${String(cutoff.getMonth() + 1).padStart(
      2,
      "0"
    )}-${String(cutoff.getDate()).padStart(2, "0")}`;
    return nights.filter((n) => n.date >= cutoffIso);
  }, [nights, range]);

  const stageAverages30 = useMemo(() => {
    const recent = nights.slice(-30).filter((n) => n.deepSec + n.lightSec + n.remSec > 0);
    if (recent.length === 0) return { Deep: null, Light: null, REM: null };
    return {
      Deep: mean(recent.map((n) => (n.deepSec / n.asleepSec) * 100)),
      Light: mean(recent.map((n) => (n.lightSec / n.asleepSec) * 100)),
      REM: mean(recent.map((n) => (n.remSec / n.asleepSec) * 100)),
    };
  }, [nights]);

  const regularity = useMemo(() => {
    const beds = shownNights
      .map((n) => n.bedtimeHours)
      .filter((v): v is number => v != null);
    const wakes = shownNights
      .map((n) => n.wakeHours)
      .filter((v): v is number => v != null);
    if (beds.length < 5 || wakes.length < 5) return null;
    const medBed = median(beds)!;
    const medWake = median(wakes)!;
    const sdBed = (stdDev(beds) ?? 0) * 60;
    const sdWake = (stdDev(wakes) ?? 0) * 60;
    return { medBed, medWake, sdBed, sdWake };
  }, [shownNights]);

  const bodyClock = useMemo(() => {
    const last60 = nights.slice(-60).filter((n) => n.midsleepHours != null && n.inBedHours != null);
    const free = last60.filter((n) => n.isWeekend);
    const work = last60.filter((n) => !n.isWeekend);
    if (free.length < 3 || work.length < 5) return null;
    const msf = mean(free.map((n) => n.midsleepHours!))!;
    const msw = mean(work.map((n) => n.midsleepHours!))!;
    const sdf = mean(free.map((n) => n.inBedHours!))!;
    const sdw = mean(work.map((n) => n.inBedHours!))!;
    const weekAvg = (5 * sdw + 2 * sdf) / 7;
    const chronotype = sdf > sdw ? msf - (sdf - weekAvg) / 2 : msf;
    const label = chronotype < 3.5 ? "Early bird" : chronotype < 5 ? "In between" : "Night owl";
    return {
      msf,
      msw,
      sdf,
      sdw,
      socialJetlagHours: msf - msw,
      chronotype,
      label,
    };
  }, [nights]);

  const sleepDebt14 = useMemo(() => {
    const recent14 = nights.slice(-14);
    const hasNativeNeed = recent14.some((n) => n.needSec != null);
    const rows = recent14.map((n) => {
      const target = n.needSec ?? 8 * 3600;
      return {
        date: n.date,
        diffHours: (n.asleepSec - target) / 3600,
        usedFallback: n.needSec == null,
      };
    });
    const owedHours = Math.max(0, -rows.reduce((s, r) => s + r.diffHours, 0));
    return { rows, owedHours, hasNativeNeed };
  }, [nights]);

  if (loading && nights.length === 0) {
    return (
      <div className="card">
        <h3 style={{ marginTop: 0 }}>Sleep</h3>
        <div className="muted">Loading sleep nights…</div>
      </div>
    );
  }

  if (nights.length === 0) {
    return (
      <div className="card">
        <h3 style={{ marginTop: 0 }}>Sleep</h3>
        <div className="muted">
          No sleep nights synced yet. Wear your watch to bed and click <strong>Sync Watch</strong> in the morning.
        </div>
      </div>
    );
  }

  const activeIdx = Math.min(nightIndex ?? nights.length - 1, nights.length - 1);
  const currentNight = nights[activeIdx];
  const isLatest = activeIdx === nights.length - 1;
  const shownAvgAsleep = mean(shownNights.map((n) => n.asleepSec));

  async function handleRegenerateTake() {
    setTakeBusy(true);
    setTakeError(null);
    try {
      const fresh = await invoke<DailyBriefingRecord>("get_or_generate_sleep_take", {
        forceRefresh: true,
      });
      onTakeUpdated(fresh);
    } catch (err) {
      setTakeError(err instanceof Error ? err.message : String(err));
    } finally {
      setTakeBusy(false);
    }
  }

  return (
    <div>
      <div className="page-header">
        <div className="page-header-left">
          <h2 className="today-title">Sleep</h2>
          <span className="today-subtitle">
            {nights.length} nights synced · Latest {currentNight.date}
          </span>
        </div>
        <div className="segmented-control">
          {([30, 90, 365] as SleepRange[]).map((d) => (
            <button
              key={d}
              type="button"
              className={`seg-btn ${range === d ? "active" : ""}`}
              onClick={() => setRange(d)}
            >
              {d === 365 ? "1 year" : `${d} nights`}
            </button>
          ))}
        </div>
      </div>

      {/* AI Sleep Take Card */}
      {(hasGeminiKey || cachedTake) && (
        <div className="insight-card sleep-tone">
          <div className="card-header" style={{ marginBottom: 8 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <Moon size={16} color="var(--chart-indigo)" />
              <strong>Your Sleep</strong>
              {cachedTake && (
                <span className="muted" style={{ fontSize: 12 }}>
                  Night of {cachedTake.date}
                </span>
              )}
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              {cachedTake && (
                <span className="muted" style={{ fontSize: 11.5 }}>
                  Written by Gemini
                </span>
              )}
              {hasGeminiKey && (
                <button
                  type="button"
                  className="secondary"
                  style={{ padding: "4px 10px", fontSize: 12, display: "inline-flex", alignItems: "center", gap: 5 }}
                  disabled={takeBusy}
                  onClick={() => void handleRegenerateTake()}
                >
                  <RefreshCw size={12} />
                  {takeBusy ? "Writing…" : "Regenerate"}
                </button>
              )}
            </div>
          </div>

          {takeBusy && !cachedTake && (
            <div className="muted" style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <Sparkles size={14} /> Reading your sleep numbers…
            </div>
          )}

          {cachedTake && (
            <>
              <div className="briefing-headline">{cachedTake.headline}</div>
              <div className="briefing-body">
                {cachedTake.body
                  .split(/\n{2,}/)
                  .filter(Boolean)
                  .map((p, i) => (
                    <p key={i}>{p}</p>
                  ))}
              </div>
              {cachedTake.action_cue && (
                <div className="action-cue-box" style={{ marginTop: 10 }}>
                  <div className="action-cue-label">Sleep Focus</div>
                  <div style={{ fontSize: 13.5 }}>{cachedTake.action_cue}</div>
                </div>
              )}
            </>
          )}
          {takeError && (
            <div className="key-status-err" style={{ marginTop: 8 }}>
              {takeError}
            </div>
          )}
        </div>
      )}

      {/* Selected Night Inspector + Stage Ranges */}
      <div className="card">
        <div className="card-header">
          <div>
            <strong>{isLatest ? "Last Night" : `Night of ${currentNight.date}`}</strong>
            <span className="muted" style={{ marginLeft: 8, fontSize: 12 }}>
              {currentNight.date}
            </span>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <button
              type="button"
              className="secondary"
              style={{ padding: "4px 8px" }}
              disabled={activeIdx === 0}
              onClick={() => setNightIndex(Math.max(0, activeIdx - 1))}
              title="Previous night"
            >
              <ChevronLeft size={15} />
            </button>
            <button
              type="button"
              className="secondary"
              style={{ padding: "4px 8px" }}
              disabled={isLatest}
              onClick={() => setNightIndex(Math.min(nights.length - 1, activeIdx + 1))}
              title="Next night"
            >
              <ChevronRight size={15} />
            </button>
          </div>
        </div>

        <div className="kpi-grid">
          <div className="kpi-tile">
            <span className="kpi-tile-label">Asleep</span>
            <span className="kpi-tile-val">{formatHm(currentNight.asleepSec)}</span>
            <span className="kpi-tile-note">
              {currentNight.needSec ? `Needed ${formatHm(currentNight.needSec)}` : "Sleep duration"}
            </span>
          </div>
          <div className="kpi-tile">
            <span className="kpi-tile-label">Sleep Score</span>
            <span className="kpi-tile-val">
              {currentNight.score != null ? `${currentNight.score}/100` : "–"}
            </span>
            <span className="kpi-tile-note">
              {currentNight.quality ? currentNight.quality.replace(/_/g, " ").toLowerCase() : "Quality"}
            </span>
          </div>
          <div className="kpi-tile">
            <span className="kpi-tile-label">Window</span>
            <span className="kpi-tile-val" style={{ fontSize: 20 }}>
              {formatClock(currentNight.bedtimeHours)} – {formatClock(currentNight.wakeHours)}
            </span>
            <span className="kpi-tile-note">
              {currentNight.efficiencyPct != null
                ? `Asleep ${Math.round(currentNight.efficiencyPct)}% of bed time`
                : "Bedtime to wake"}
            </span>
          </div>
          <div className="kpi-tile">
            <span className="kpi-tile-label">Physiology</span>
            <span className="kpi-tile-val" style={{ fontSize: 20 }}>
              {currentNight.avgHr != null
                ? `${Math.round(currentNight.avgHr)} bpm`
                : currentNight.respirationAvg != null
                ? `${Math.round(currentNight.respirationAvg)} brpm`
                : "–"}
            </span>
            <span className="kpi-tile-note">
              {[
                currentNight.sleepStress != null ? `Stress ${Math.round(currentNight.sleepStress)}` : null,
                currentNight.awakeCount != null ? `Woke ${currentNight.awakeCount}×` : null,
                currentNight.respirationLow != null && currentNight.respirationHigh != null
                  ? `Resp ${Math.round(currentNight.respirationLow)}–${Math.round(currentNight.respirationHigh)}`
                  : null,
              ]
                .filter(Boolean)
                .join(" · ") || "Overnight vitals"}
            </span>
          </div>
        </div>

        {/* Stages vs Optimal Range */}
        {currentNight.deepSec + currentNight.lightSec + currentNight.remSec > 0 ? (
          <div style={{ marginTop: 8 }}>
            <div className="muted" style={{ fontSize: 12, marginBottom: 10 }}>
              Sleep stages vs Garmin optimal ranges (vertical tick = 30-night average)
            </div>
            {OPTIMAL_STAGES.map((opt) => {
              const sec =
                opt.stage === "Deep"
                  ? currentNight.deepSec
                  : opt.stage === "Light"
                  ? currentNight.lightSec
                  : currentNight.remSec;
              const share = currentNight.asleepSec > 0 ? (sec / currentNight.asleepSec) * 100 : 0;
              const avgShare = stageAverages30[opt.stage];
              const verdict =
                share < opt.low ? "below range" : share > opt.high ? "above range" : "in range";
              const outOfRange = share < opt.low || share > opt.high;
              const toPct = (v: number) => `${Math.min(100, Math.max(0, (v / 70) * 100))}%`;
              return (
                <div key={opt.stage} className="stage-range-row">
                  <div>
                    <strong style={{ fontSize: 13 }}>{opt.stage}</strong>
                    <div className="muted" style={{ fontSize: 11.5 }}>
                      {formatHm(sec)} · {Math.round(share)}%
                    </div>
                  </div>
                  <div className="stage-bar-track">
                    <div className="stage-bar-bg" />
                    <div
                      className="stage-optimal-band"
                      style={{
                        left: toPct(opt.low),
                        width: `${((opt.high - opt.low) / 70) * 100}%`,
                        background: opt.color,
                      }}
                    />
                    <div
                      className="stage-bar-fill"
                      style={{
                        width: toPct(Math.max(2, share)),
                        background: opt.color,
                      }}
                    />
                    {avgShare != null && (
                      <div
                        className="stage-avg-tick"
                        style={{ left: toPct(avgShare) }}
                        title={`30-night average: ${Math.round(avgShare)}%`}
                      />
                    )}
                  </div>
                  <div
                    style={{
                      textAlign: "right",
                      fontSize: 12,
                      color: outOfRange ? "var(--amber)" : "var(--muted)",
                    }}
                  >
                    {verdict}
                  </div>
                </div>
              );
            })}
            {currentNight.awakeSec > 0 && (
              <div className="muted" style={{ fontSize: 12, marginTop: 4 }}>
                Awake {formatHm(currentNight.awakeSec)} during the night.
              </div>
            )}
          </div>
        ) : (
          <div className="muted" style={{ fontSize: 12.5 }}>
            No stage breakdown recorded for this night.
          </div>
        )}
      </div>

      {/* Sleep Need vs Actual (14-Night Sleep Debt) */}
      <div className="card">
        <div className="card-header">
          <div>
            <strong>Sleep Need vs. Actual (Last 14 Nights)</strong>
            <div className="muted" style={{ fontSize: 12 }}>
              {sleepDebt14.owedHours < 0.5
                ? "No sleep debt over the last 14 nights"
                : `${formatHm(sleepDebt14.owedHours * 3600)} short over the last 14 nights`}
              {!sleepDebt14.hasNativeNeed ? " · Using 8h reference where watch sleep need was absent" : ""}
            </div>
          </div>
        </div>
        <svg viewBox="0 0 860 175" style={{ width: "100%", height: "auto", display: "block" }}>
          <line x1={44} y1={88} x2={840} y2={88} stroke="var(--muted)" strokeOpacity={0.55} />
          <text x={38} y={91} textAnchor="end" fontSize={9.5} fill="var(--muted)">
            need
          </text>
          <text x={38} y={24} textAnchor="end" fontSize={9.5} fill="var(--muted)">
            +2h
          </text>
          <text x={38} y={156} textAnchor="end" fontSize={9.5} fill="var(--muted)">
            -2h
          </text>
          {sleepDebt14.rows.map((r, idx) => {
            const slotW = (840 - 48) / Math.max(1, sleepDebt14.rows.length);
            const barW = Math.min(34, slotW * 0.68);
            const x = 48 + idx * slotW + (slotW - barW) / 2;
            const clamped = Math.max(-2.5, Math.min(2.5, r.diffHours));
            const px = (Math.abs(clamped) / 2.5) * 68;
            const y = clamped >= 0 ? 88 - px : 88;
            return (
              <g key={r.date}>
                <rect
                  x={x}
                  y={y}
                  width={barW}
                  height={Math.max(2, px)}
                  rx={3}
                  fill={clamped < 0 ? "var(--amber)" : "var(--chart-indigo)"}
                >
                  <title>{`${r.date}: ${clamped >= 0 ? "+" : ""}${r.diffHours.toFixed(1)} h vs need`}</title>
                </rect>
                <text x={x + barW / 2} y={169} textAnchor="middle" fontSize={8.5} fill="var(--muted)">
                  {r.date.slice(5)}
                </text>
              </g>
            );
          })}
        </svg>
      </div>

      {/* Nightly Stacked Stages & Score */}
      <div className="card">
        <div className="card-header">
          <div>
            <strong>Nightly Sleep Duration & Stages</strong>
            <div className="muted" style={{ fontSize: 12 }}>
              Average {formatHm(shownAvgAsleep)} asleep across {shownNights.length} nights
            </div>
          </div>
        </div>
        {(() => {
          const maxH = Math.max(
            9,
            ...shownNights.map((n) => (n.asleepSec + n.awakeSec) / 3600)
          );
          const avgH = shownAvgAsleep ? shownAvgAsleep / 3600 : null;
          return (
            <div className="svg-chart-wrap">
              <svg viewBox="0 0 880 215" style={{ width: "100%", height: "auto", display: "block" }}>
                {[2, 4, 6, 8, 10]
                  .filter((h) => h <= maxH + 0.5)
                  .map((h) => {
                    const y = 180 - (h / maxH) * 155;
                    return (
                      <g key={h}>
                        <line x1={38} y1={y} x2={855} y2={y} stroke="var(--border)" strokeDasharray="3 3" />
                        <text x={32} y={y + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                          {h}h
                        </text>
                      </g>
                    );
                  })}
                {shownNights.map((n, idx) => {
                  const slotW = (855 - 42) / Math.max(1, shownNights.length);
                  const barW = Math.max(2, Math.min(20, slotW * 0.72));
                  const x = 42 + idx * slotW + (slotW - barW) / 2;
                  const hasStages = n.deepSec + n.lightSec + n.remSec > 0;
                  const segments = hasStages
                    ? [
                        { sec: n.deepSec, color: "var(--chart-indigo)" },
                        { sec: n.lightSec, color: "var(--chart-blue)" },
                        { sec: n.remSec, color: "var(--chart-purple)" },
                        { sec: n.awakeSec, color: "var(--chart-pink)" },
                      ]
                    : [{ sec: n.asleepSec, color: "var(--chart-indigo)" }];
                  let curY = 180;
                  return (
                    <g key={n.date}>
                      {segments.map((seg, si) => {
                        if (seg.sec <= 0) return null;
                        const h = (seg.sec / 3600 / maxH) * 155;
                        curY -= h;
                        return (
                          <rect
                            key={si}
                            x={x}
                            y={curY}
                            width={barW}
                            height={Math.max(1, h)}
                            fill={seg.color}
                            rx={1}
                          >
                            <title>{`${n.date}: ${formatHm(n.asleepSec)}${
                              n.score != null ? ` · Score ${n.score}` : ""
                            }`}</title>
                          </rect>
                        );
                      })}
                    </g>
                  );
                })}
                {avgH != null && (
                  <line
                    x1={42}
                    y1={180 - (avgH / maxH) * 155}
                    x2={855}
                    y2={180 - (avgH / maxH) * 155}
                    stroke="var(--text)"
                    strokeOpacity={0.55}
                    strokeDasharray="4 3"
                  />
                )}
                {/* Score line overlay (0..100 mapped onto chart height) */}
                {shownNights.filter((n) => n.score != null).length > 1 && (
                  <polyline
                    fill="none"
                    stroke="var(--accent-primary)"
                    strokeWidth={1.8}
                    opacity={0.85}
                    points={shownNights
                      .map((n, idx) => {
                        if (n.score == null) return null;
                        const slotW = (855 - 42) / Math.max(1, shownNights.length);
                        const x = 42 + idx * slotW + slotW / 2;
                        const y = 180 - (n.score / 100) * 155;
                        return `${x.toFixed(1)},${y.toFixed(1)}`;
                      })
                      .filter(Boolean)
                      .join(" ")}
                  />
                )}
                {shownNights.length > 0 && (
                  <>
                    <text x={42} y={200} fontSize={9.5} fill="var(--muted)">
                      {shownNights[0].date}
                    </text>
                    <text x={855} y={200} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                      {shownNights[shownNights.length - 1].date}
                    </text>
                  </>
                )}
              </svg>
              <div className="chart-legend">
                <span className="legend-item">
                  <span className="legend-swatch" style={{ background: "var(--chart-indigo)" }} /> Deep
                </span>
                <span className="legend-item">
                  <span className="legend-swatch" style={{ background: "var(--chart-blue)" }} /> Light
                </span>
                <span className="legend-item">
                  <span className="legend-swatch" style={{ background: "var(--chart-purple)" }} /> REM
                </span>
                <span className="legend-item">
                  <span className="legend-swatch" style={{ background: "var(--chart-pink)" }} /> Awake
                </span>
                <span className="legend-item">
                  <span className="legend-line" style={{ background: "var(--accent-primary)" }} /> Sleep Score (0–100)
                </span>
              </div>
            </div>
          );
        })()}
      </div>

      {/* Bedtime/Wake Consistency & Body Clock */}
      <div className="two-col-grid">
        <div className="card" style={{ marginBottom: 0 }}>
          <div className="card-header">
            <div>
              <strong>When You Sleep</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                {regularity
                  ? `Usual ${formatClock(regularity.medBed)}–${formatClock(regularity.medWake)} (± ${Math.round(
                      regularity.sdBed
                    )} min bedtime spread)`
                  : "Bedtime & wake-up consistency"}
              </div>
            </div>
          </div>
          {(() => {
            const windowed = shownNights.filter(
              (n) => n.bedtimeHours != null && n.wakeHours != null
            );
            if (windowed.length < 2) {
              return <div className="muted">Not enough sleep timestamps synced yet.</div>;
            }
            const low = Math.floor(Math.min(...windowed.map((n) => n.bedtimeHours!))) - 0.5;
            const high = Math.ceil(Math.max(...windowed.map((n) => n.wakeHours!))) + 0.5;
            const span = Math.max(4, high - low);
            const toY = (h: number) => 16 + ((h - low) / span) * 155;
            return (
              <svg viewBox="0 0 640 195" style={{ width: "100%", height: "auto", display: "block" }}>
                {Array.from(
                  { length: Math.ceil(high) - Math.floor(low) + 1 },
                  (_, i) => Math.floor(low) + i
                )
                  .filter((h) => h % 2 === 0)
                  .map((h) => {
                    const y = toY(h);
                    return (
                      <g key={h}>
                        <line x1={46} y1={y} x2={624} y2={y} stroke="var(--border)" strokeDasharray="3 3" />
                        <text x={40} y={y + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                          {formatClock(h)}
                        </text>
                      </g>
                    );
                  })}
                {windowed.map((n, idx) => {
                  const slotW = (624 - 50) / Math.max(1, windowed.length);
                  const barW = Math.max(3, Math.min(16, slotW * 0.68));
                  const x = 50 + idx * slotW + (slotW - barW) / 2;
                  const y1 = toY(n.bedtimeHours!);
                  const y2 = toY(n.wakeHours!);
                  return (
                    <rect
                      key={n.date}
                      x={x}
                      y={y1}
                      width={barW}
                      height={Math.max(3, y2 - y1)}
                      rx={2.5}
                      fill={n.isWeekend ? "var(--chart-purple)" : "var(--chart-indigo)"}
                      opacity={n.isWeekend ? 0.78 : 0.92}
                    >
                      <title>{`${n.date}: ${formatClock(n.bedtimeHours)}–${formatClock(n.wakeHours)}`}</title>
                    </rect>
                  );
                })}
                {regularity && (
                  <>
                    <line
                      x1={50}
                      y1={toY(regularity.medBed)}
                      x2={624}
                      y2={toY(regularity.medBed)}
                      stroke="var(--text)"
                      strokeOpacity={0.5}
                      strokeDasharray="4 3"
                    />
                    <line
                      x1={50}
                      y1={toY(regularity.medWake)}
                      x2={624}
                      y2={toY(regularity.medWake)}
                      stroke="var(--text)"
                      strokeOpacity={0.5}
                      strokeDasharray="4 3"
                    />
                  </>
                )}
                <text x={50} y={187} fontSize={9.5} fill="var(--muted)">
                  {windowed[0].date}
                </text>
                <text x={624} y={187} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                  {windowed[windowed.length - 1].date}
                </text>
              </svg>
            );
          })()}
        </div>

        <div className="card" style={{ marginBottom: 0 }}>
          <div className="card-header">
            <div>
              <strong>Your Body Clock</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                Last 60 nights · Weekdays vs weekends
              </div>
            </div>
          </div>
          {!bodyClock ? (
            <div className="muted">
              Needs at least 5 weekday and 3 weekend nights with sleep timestamps.
            </div>
          ) : (
            <div>
              <div className="kpi-grid" style={{ gridTemplateColumns: "1fr 1fr", marginBottom: 12 }}>
                <div className="kpi-tile">
                  <span className="kpi-tile-label">Social Jetlag</span>
                  <span className="kpi-tile-val" style={{ fontSize: 21 }}>
                    {Math.round(Math.abs(bodyClock.socialJetlagHours) * 60)} min
                  </span>
                  <span className="kpi-tile-note">
                    {bodyClock.socialJetlagHours >= 0 ? "later on weekends" : "earlier on weekends"}
                  </span>
                </div>
                <div className="kpi-tile">
                  <span className="kpi-tile-label">Chronotype</span>
                  <span className="kpi-tile-val" style={{ fontSize: 21 }}>
                    {bodyClock.label}
                  </span>
                  <span className="kpi-tile-note">
                    Mid-sleep {formatClock(bodyClock.chronotype)} on free days
                  </span>
                </div>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                {[
                  {
                    label: "Weekdays",
                    mid: bodyClock.msw,
                    dur: bodyClock.sdw,
                    color: "var(--chart-indigo)",
                  },
                  {
                    label: "Weekends",
                    mid: bodyClock.msf,
                    dur: bodyClock.sdf,
                    color: "var(--chart-purple)",
                  },
                ].map((row) => {
                  const start = row.mid - row.dur / 2;
                  const end = row.mid + row.dur / 2;
                  return (
                    <div key={row.label}>
                      <div style={{ display: "flex", justifyContent: "space-between", fontSize: 12.5, marginBottom: 4 }}>
                        <strong>{row.label}</strong>
                        <span className="muted">
                          {formatClock(start)} – {formatClock(end)} (mid {formatClock(row.mid)})
                        </span>
                      </div>
                      <div className="envelope-bar-track" style={{ height: 10, position: "relative" }}>
                        <div
                          style={{
                            position: "absolute",
                            left: `${Math.max(0, ((start + 4) / 16) * 100)}%`,
                            width: `${Math.min(100, (row.dur / 16) * 100)}%`,
                            height: "100%",
                            borderRadius: 999,
                            background: row.color,
                          }}
                        />
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
