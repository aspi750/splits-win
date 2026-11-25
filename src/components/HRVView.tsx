import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Activity as ActivityIcon, ChevronLeft, ChevronRight, RefreshCw, Sparkles } from "lucide-react";
import type { ActivityRecord, DailyBriefingRecord, DayRecord, NightHrvDetail } from "../types";

type HrvRange = 30 | 90 | 180 | 365;

interface HRVViewProps {
  days: DayRecord[];
  activities: ActivityRecord[];
  hasGarminKey: boolean;
  hasGeminiKey: boolean;
  loading: boolean;
  cachedTake: DailyBriefingRecord | null;
  onTakeUpdated: (take: DailyBriefingRecord) => void;
}

interface HrvSeriesPoint {
  date: string;
  ms: number;
  mean7Ms: number;
  cvPct: number;
  usualCvPct: number | null;
  bandLowMs: number | null;
  bandHighMs: number | null;
  status: string | null;
  fiveMinHigh: number | null;
  restingHr: number | null;
  sleepSec: number | null;
}

function mean(xs: number[]): number | null {
  return xs.length === 0 ? null : xs.reduce((a, b) => a + b, 0) / xs.length;
}

function stdDev(xs: number[]): number | null {
  if (xs.length < 2) return null;
  const m = mean(xs)!;
  const v = xs.reduce((s, x) => s + (x - m) ** 2, 0) / (xs.length - 1);
  return Math.sqrt(v);
}

function formatHrvBadge(status: string | null | undefined): {
  label: string;
  cls: "green" | "amber" | "red";
} {
  if (!status || !status.trim()) return { label: "No Status", cls: "amber" };
  const up = status.trim().toUpperCase();
  const label = up
    .split(/[_\s]+/)
    .map((w) => w.charAt(0) + w.slice(1).toLowerCase())
    .join(" ");
  if (up === "BALANCED" || up === "OPTIMAL" || up === "GOOD") return { label, cls: "green" };
  if (up === "LOW" || up === "POOR" || up === "STRAINED") return { label, cls: "red" };
  return { label, cls: "amber" };
}

function formatHm(sec: number | null | undefined): string {
  if (!sec || sec <= 0) return "–";
  const mins = Math.round(sec / 60);
  if (mins >= 60) return `${Math.floor(mins / 60)}h ${mins % 60}m`;
  return `${mins}m`;
}

export function HRVView({
  days,
  activities,
  hasGarminKey,
  hasGeminiKey,
  loading,
  cachedTake,
  onTakeUpdated,
}: HRVViewProps) {
  const [range, setRange] = useState<HrvRange>(90);
  const [nightIndex, setNightIndex] = useState<number | null>(null);
  const [nightDetail, setNightDetail] = useState<NightHrvDetail | null>(null);
  const [nightLoading, setNightLoading] = useState(false);
  const [takeBusy, setTakeBusy] = useState(false);
  const [takeError, setTakeError] = useState<string | null>(null);
  const nightCacheRef = useRef<Record<string, NightHrvDetail>>({});
  const attemptedTakeRef = useRef<string | null>(null);

  const series = useMemo<HrvSeriesPoint[]>(() => {
    const sorted = [...days]
      .filter((d) => d.hrv_last_night != null && d.hrv_last_night > 0)
      .sort((a, b) => a.date.localeCompare(b.date));

    const out: HrvSeriesPoint[] = [];
    for (let i = 0; i < sorted.length; i++) {
      const d = sorted[i];
      const ms = d.hrv_last_night!;
      const win7 = sorted.slice(Math.max(0, i - 6), i + 1);
      const lns7 = win7.map((x) => Math.log(x.hrv_last_night!));
      const mLn = mean(lns7) ?? Math.log(ms);
      const sdLn = stdDev(lns7) ?? 0;
      const cvPct = mLn > 0 ? (sdLn / mLn) * 100 : 0;

      const baseSlice = sorted.slice(Math.max(0, i - 60), i);
      let bandLowMs = d.hrv_baseline_low ?? null;
      let bandHighMs = d.hrv_baseline_high ?? null;
      if ((bandLowMs == null || bandHighMs == null) && baseSlice.length >= 20) {
        const bLns = baseSlice.map((x) => Math.log(x.hrv_last_night!));
        const bm = mean(bLns);
        const bsd = stdDev(bLns);
        if (bm != null && bsd != null) {
          bandLowMs = Math.exp(bm - 0.5 * bsd);
          bandHighMs = Math.exp(bm + 0.5 * bsd);
        }
      }
      const prevCvs = out.slice(Math.max(0, out.length - 60)).map((p) => p.cvPct);
      const usualCvPct = prevCvs.length >= 10 ? mean(prevCvs) : null;

      out.push({
        date: d.date,
        ms,
        mean7Ms: d.hrv_weekly_avg ?? Math.exp(mLn),
        cvPct,
        usualCvPct,
        bandLowMs,
        bandHighMs,
        status: d.hrv_status ?? null,
        fiveMinHigh: d.hrv_last_night_5min_high ?? null,
        restingHr: d.resting_hr ?? null,
        sleepSec: d.sleep_seconds ?? d.sleep_duration_sec ?? null,
      });
    }
    return out;
  }, [days]);

  const latestHrvDate = series.length > 0 ? series[series.length - 1].date : null;
  const activeNightIdx = series.length > 0 ? Math.min(nightIndex ?? series.length - 1, series.length - 1) : 0;
  const selectedNightDate = series[activeNightIdx]?.date ?? null;

  useEffect(() => {
    if (!hasGeminiKey || !latestHrvDate) return;
    if (cachedTake && cachedTake.date === latestHrvDate) return;
    if (attemptedTakeRef.current === latestHrvDate) return;
    attemptedTakeRef.current = latestHrvDate;
    let cancelled = false;
    setTakeBusy(true);
    setTakeError(null);
    void invoke<DailyBriefingRecord>("get_or_generate_hrv_take", { forceRefresh: false })
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
  }, [hasGeminiKey, latestHrvDate, cachedTake, onTakeUpdated]);

  useEffect(() => {
    if (!selectedNightDate) {
      setNightDetail(null);
      return;
    }
    const cached = nightCacheRef.current[selectedNightDate];
    if (cached) {
      setNightDetail(cached);
      return;
    }
    if (!hasGarminKey) {
      setNightDetail(null);
      return;
    }
    let cancelled = false;
    setNightLoading(true);
    void invoke<NightHrvDetail>("get_night_hrv_readings", { date: selectedNightDate })
      .then((res) => {
        if (cancelled) return;
        nightCacheRef.current[selectedNightDate] = res;
        setNightDetail(res);
      })
      .catch(() => {
        if (!cancelled) setNightDetail(null);
      })
      .finally(() => {
        if (!cancelled) setNightLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedNightDate, hasGarminKey]);

  const shownSeries = useMemo(() => {
    if (series.length === 0) return [];
    const cutoff = new Date();
    cutoff.setDate(cutoff.getDate() - range);
    const cutoffIso = `${cutoff.getFullYear()}-${String(cutoff.getMonth() + 1).padStart(
      2,
      "0"
    )}-${String(cutoff.getDate()).padStart(2, "0")}`;
    return series.filter((p) => p.date >= cutoffIso);
  }, [series, range]);

  const loadRatio = useMemo(() => {
    const now = new Date();
    const d7 = new Date(now);
    d7.setDate(now.getDate() - 6);
    const d28 = new Date(now);
    d28.setDate(now.getDate() - 27);
    const iso7 = `${d7.getFullYear()}-${String(d7.getMonth() + 1).padStart(2, "0")}-${String(
      d7.getDate()
    ).padStart(2, "0")}`;
    const iso28 = `${d28.getFullYear()}-${String(d28.getMonth() + 1).padStart(2, "0")}-${String(
      d28.getDate()
    ).padStart(2, "0")}`;
    const acute = activities
      .filter((a) => a.start_iso.slice(0, 10) >= iso7)
      .reduce((s, a) => s + (a.training_load ?? 0), 0);
    const chronic =
      activities
        .filter((a) => a.start_iso.slice(0, 10) >= iso28)
        .reduce((s, a) => s + (a.training_load ?? 0), 0) / 4;
    return chronic > 0 ? acute / chronic : null;
  }, [activities]);

  // Quadrant z-scores (HRV vs RHR, last 28 paired nights)
  const quadrantPairs = useMemo(() => {
    const sorted = [...days].sort((a, b) => a.date.localeCompare(b.date));
    const pairs: { date: string; hrvZ: number; rhrZ: number }[] = [];
    for (let i = 0; i < sorted.length; i++) {
      const d = sorted[i];
      if (!d.hrv_last_night || d.hrv_last_night <= 0 || !d.resting_hr || d.resting_hr <= 0) continue;
      const base = sorted.slice(Math.max(0, i - 60), i);
      const lns = base
        .map((x) => x.hrv_last_night)
        .filter((v): v is number => v != null && v > 0)
        .map((v) => Math.log(v));
      const rhrs = base
        .map((x) => x.resting_hr)
        .filter((v): v is number => v != null && v > 0);
      if (lns.length < 14 || rhrs.length < 14) continue;
      const lm = mean(lns)!;
      const ls = stdDev(lns)!;
      const rm = mean(rhrs)!;
      const rs = stdDev(rhrs)!;
      if (ls <= 0 || rs <= 0) continue;
      pairs.push({
        date: d.date,
        hrvZ: Math.max(-3, Math.min(3, (Math.log(d.hrv_last_night) - lm) / ls)),
        rhrZ: Math.max(-3, Math.min(3, (d.resting_hr - rm) / rs)),
      });
    }
    return pairs.slice(-28);
  }, [days]);

  if (loading && series.length === 0) {
    return (
      <div className="card">
        <h3 style={{ marginTop: 0 }}>HRV</h3>
        <div className="muted">Loading HRV data…</div>
      </div>
    );
  }

  if (series.length === 0) {
    return (
      <div className="card">
        <h3 style={{ marginTop: 0 }}>HRV</h3>
        <div className="muted">
          No overnight HRV readings synced yet. Wear your watch to bed and click <strong>Sync Watch</strong>.
        </div>
      </div>
    );
  }

  const latest = series[series.length - 1];
  const latestBadge = formatHrvBadge(latest.status);
  const recentRhrs = series
    .slice(-7)
    .map((p) => p.restingHr)
    .filter((v): v is number => v != null && v > 0);
  const avg7Rhr = recentRhrs.length > 0 ? Math.round(mean(recentRhrs)!) : null;
  const selectedNight = series[activeNightIdx];

  async function handleRegenerateTake() {
    setTakeBusy(true);
    setTakeError(null);
    try {
      const fresh = await invoke<DailyBriefingRecord>("get_or_generate_hrv_take", {
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
          <h2 className="today-title">HRV</h2>
          <span className="today-subtitle">
            {series.length} nights tracked · Latest {latest.date}
          </span>
        </div>
        <div className="segmented-control">
          {([30, 90, 180, 365] as HrvRange[]).map((d) => (
            <button
              key={d}
              type="button"
              className={`seg-btn ${range === d ? "active" : ""}`}
              onClick={() => setRange(d)}
            >
              {d === 365 ? "1Y" : `${d}D`}
            </button>
          ))}
        </div>
      </div>

      {/* AI HRV Take Card */}
      {(hasGeminiKey || cachedTake) && (
        <div className="insight-card hrv-tone">
          <div className="card-header" style={{ marginBottom: 8 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <ActivityIcon size={16} color="var(--chart-teal)" />
              <strong>Your HRV</strong>
              {cachedTake && (
                <span className="muted" style={{ fontSize: 12 }}>
                  Morning of {cachedTake.date}
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
              <Sparkles size={14} /> Reading your HRV baseline…
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
                  <div className="action-cue-label">Autonomic Guidance</div>
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

      {/* Status Summary KPI Row */}
      <div className="kpi-grid">
        <div className="kpi-tile">
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
            <span className="kpi-tile-label">Last Night</span>
            <span className={`pill ${latestBadge.cls}`} style={{ fontSize: 11, padding: "2px 8px" }}>
              {latestBadge.label}
            </span>
          </div>
          <span className="kpi-tile-val">{latest.ms} ms</span>
          <span className="kpi-tile-note">
            {latest.fiveMinHigh != null ? `5-min high: ${latest.fiveMinHigh} ms` : latest.date}
          </span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">7-Day Average</span>
          <span className="kpi-tile-val">{Math.round(latest.mean7Ms)} ms</span>
          <span className="kpi-tile-note">
            {latest.bandLowMs != null && latest.bandHighMs != null
              ? `Baseline ${Math.round(latest.bandLowMs)}–${Math.round(latest.bandHighMs)} ms`
              : "Baseline builds over 60 nights"}
          </span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Day-to-Day Swing</span>
          <span className="kpi-tile-val">{latest.cvPct.toFixed(1)}%</span>
          <span className="kpi-tile-note">
            {latest.usualCvPct != null ? `Usually ${latest.usualCvPct.toFixed(1)}%` : "7-day CV"}
          </span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Resting HR</span>
          <span className="kpi-tile-val">{avg7Rhr != null ? `${avg7Rhr} bpm` : "–"}</span>
          <span className="kpi-tile-note">7-day average</span>
        </div>
        {loadRatio != null && (
          <div className="kpi-tile">
            <span className="kpi-tile-label">Training Load</span>
            <span className="kpi-tile-val">{loadRatio.toFixed(1)}×</span>
            <span className="kpi-tile-note">7d vs usual week</span>
          </div>
        )}
      </div>

      {/* Multi-Week Overnight HRV Trend against Shaded Balanced Band */}
      <div className="card">
        <div className="card-header">
          <div>
            <strong>Against Your Normal Baseline</strong>
            <div className="muted" style={{ fontSize: 12 }}>
              7-day rolling HRV curve against your balanced baseline band
            </div>
          </div>
        </div>
        {shownSeries.length < 2 ? (
          <div className="muted">Not enough nights in this timeframe.</div>
        ) : (
          (() => {
            const allNums = shownSeries.flatMap((p) =>
              [p.ms, p.mean7Ms, p.bandLowMs, p.bandHighMs].filter((v): v is number => v != null)
            );
            const lo = Math.max(0, Math.min(...allNums) - 5);
            const hi = Math.max(...allNums) + 5;
            const span = Math.max(10, hi - lo);
            const toX = (i: number) => 44 + (i / Math.max(1, shownSeries.length - 1)) * 800;
            const toY = (v: number) => 185 - ((v - lo) / span) * 160;

            // Build contiguous baseline band segments
            const bandSegments: HrvSeriesPoint[][] = [];
            let curSeg: HrvSeriesPoint[] = [];
            const indexed = shownSeries.map((p, idx) => ({ ...p, idx }));
            let segIdx: (HrvSeriesPoint & { idx: number })[][] = [];
            let curIdxSeg: (HrvSeriesPoint & { idx: number })[] = [];
            for (const p of indexed) {
              if (p.bandLowMs != null && p.bandHighMs != null) {
                curIdxSeg.push(p);
                curSeg.push(p);
              } else if (curIdxSeg.length > 0) {
                segIdx.push(curIdxSeg);
                bandSegments.push(curSeg);
                curIdxSeg = [];
                curSeg = [];
              }
            }
            if (curIdxSeg.length > 0) {
              segIdx.push(curIdxSeg);
              bandSegments.push(curSeg);
            }

            return (
              <div className="svg-chart-wrap">
                <svg viewBox="0 0 880 215" style={{ width: "100%", height: "auto", display: "block" }}>
                  {[0, 0.25, 0.5, 0.75, 1].map((f) => {
                    const v = lo + f * span;
                    const y = toY(v);
                    return (
                      <g key={f}>
                        <line x1={44} y1={y} x2={844} y2={y} stroke="var(--border)" strokeDasharray="3 3" />
                        <text x={38} y={y + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                          {Math.round(v)}
                        </text>
                      </g>
                    );
                  })}
                  {segIdx.map((seg, si) => {
                    if (seg.length < 2) return null;
                    const topPts = seg.map((p) => `${toX(p.idx).toFixed(1)},${toY(p.bandHighMs!).toFixed(1)}`);
                    const botPts = [...seg]
                      .reverse()
                      .map((p) => `${toX(p.idx).toFixed(1)},${toY(p.bandLowMs!).toFixed(1)}`);
                    return (
                      <polygon
                        key={si}
                        points={[...topPts, ...botPts].join(" ")}
                        fill="var(--chart-teal)"
                        fillOpacity={0.16}
                      />
                    );
                  })}
                  {shownSeries.map((p, i) => (
                    <circle key={p.date} cx={toX(i)} cy={toY(p.ms)} r={2.8} fill="var(--chart-teal)" fillOpacity={0.45}>
                      <title>{`${p.date}: ${p.ms} ms (7d avg ${Math.round(p.mean7Ms)} ms)`}</title>
                    </circle>
                  ))}
                  <polyline
                    fill="none"
                    stroke="var(--chart-teal)"
                    strokeWidth={2.6}
                    points={shownSeries
                      .map((p, i) => `${toX(i).toFixed(1)},${toY(p.mean7Ms).toFixed(1)}`)
                      .join(" ")}
                  />
                  <text x={44} y={205} fontSize={9.5} fill="var(--muted)">
                    {shownSeries[0].date}
                  </text>
                  <text x={844} y={205} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                    {shownSeries[shownSeries.length - 1].date}
                  </text>
                </svg>
                <div className="chart-legend">
                  <span className="legend-item">
                    <span className="legend-line" style={{ background: "var(--chart-teal)" }} /> 7-day average
                  </span>
                  <span className="legend-item">
                    <span className="legend-swatch" style={{ background: "var(--chart-teal)", opacity: 0.45 }} /> Nightly reading
                  </span>
                  <span className="legend-item">
                    <span className="legend-swatch" style={{ background: "var(--chart-teal)", opacity: 0.18 }} /> Balanced baseline band
                  </span>
                </div>
              </div>
            );
          })()
        )}
      </div>

      {/* Last Night 5-Min Intraday Curve */}
      <div className="card">
        <div className="card-header">
          <div>
            <strong>
              {activeNightIdx === series.length - 1
                ? "Last Night (5-Min Overnight HRV)"
                : `Night of ${selectedNight.date}`}
            </strong>
            <div className="muted" style={{ fontSize: 12 }}>
              Avg {selectedNight.ms} ms
              {selectedNight.fiveMinHigh != null ? ` · 5-min peak ${selectedNight.fiveMinHigh} ms` : ""}
            </div>
          </div>
          <div style={{ display: "flex", gap: 6 }}>
            <button
              type="button"
              className="secondary"
              style={{ padding: "4px 8px" }}
              disabled={activeNightIdx === 0}
              onClick={() => setNightIndex(Math.max(0, activeNightIdx - 1))}
            >
              <ChevronLeft size={15} />
            </button>
            <button
              type="button"
              className="secondary"
              style={{ padding: "4px 8px" }}
              disabled={activeNightIdx === series.length - 1}
              onClick={() => setNightIndex(Math.min(series.length - 1, activeNightIdx + 1))}
            >
              <ChevronRight size={15} />
            </button>
          </div>
        </div>
        {nightLoading ? (
          <div className="muted">Fetching 5-minute overnight readings from Garmin…</div>
        ) : nightDetail && nightDetail.readings.length >= 2 ? (
          (() => {
            const pts = nightDetail.readings;
            const vals = pts.map((r) => r.value_ms);
            const lo = Math.max(0, Math.min(...vals) - 5);
            const hi = Math.max(...vals) + 6;
            const span = Math.max(10, hi - lo);
            const toX = (i: number) => 42 + (i / (pts.length - 1)) * 800;
            const toY = (v: number) => 170 - ((v - lo) / span) * 145;
            const avgY = toY(selectedNight.ms);
            const linePts = pts.map((p, i) => `${toX(i).toFixed(1)},${toY(p.value_ms).toFixed(1)}`).join(" ");
            const areaPts = `${linePts} ${toX(pts.length - 1).toFixed(1)},170 ${toX(0).toFixed(1)},170`;
            return (
              <svg viewBox="0 0 880 195" style={{ width: "100%", height: "auto", display: "block" }}>
                {[0, 0.5, 1].map((f) => {
                  const v = lo + f * span;
                  const y = toY(v);
                  return (
                    <g key={f}>
                      <line x1={42} y1={y} x2={842} y2={y} stroke="var(--border)" strokeDasharray="3 3" />
                      <text x={36} y={y + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                        {Math.round(v)}
                      </text>
                    </g>
                  );
                })}
                <polygon points={areaPts} fill="var(--chart-teal)" fillOpacity={0.18} />
                <polyline fill="none" stroke="var(--chart-teal)" strokeWidth={2.2} points={linePts} />
                <line x1={42} y1={avgY} x2={842} y2={avgY} stroke="var(--muted)" strokeDasharray="4 3" />
                <text x={46} y={avgY - 4} fontSize={9.5} fill="var(--muted)">
                  Avg {selectedNight.ms} ms
                </text>
                <text x={42} y={187} fontSize={9.5} fill="var(--muted)">
                  {pts[0].timestamp_iso.slice(11, 16)}
                </text>
                <text x={842} y={187} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                  {pts[pts.length - 1].timestamp_iso.slice(11, 16)}
                </text>
              </svg>
            );
          })()
        ) : (
          <div className="muted">
            {hasGarminKey
              ? "No 5-minute intraday readings returned for this night."
              : "Connect your Garmin key to view 5-minute overnight HRV curves."}
          </div>
        )}
      </div>

      {/* Day-to-Day Swing & HRV vs RHR Quadrant */}
      <div className="two-col-grid">
        <div className="card" style={{ marginBottom: 0 }}>
          <div className="card-header">
            <div>
              <strong>Day-to-Day Swing (CV%)</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                7-day coefficient of variation in ln(rMSSD)
              </div>
            </div>
          </div>
          {shownSeries.length < 4 ? (
            <div className="muted">Not enough nights in this range.</div>
          ) : (
            (() => {
              const maxCv = Math.max(5, ...shownSeries.map((p) => p.cvPct)) * 1.15;
              const toX = (i: number) => 40 + (i / Math.max(1, shownSeries.length - 1)) * 580;
              const toY = (v: number) => 165 - (v / maxCv) * 145;
              return (
                <svg viewBox="0 0 640 190" style={{ width: "100%", height: "auto", display: "block" }}>
                  {[0, 0.5, 1].map((f) => {
                    const v = f * maxCv;
                    const y = toY(v);
                    return (
                      <g key={f}>
                        <line x1={40} y1={y} x2={620} y2={y} stroke="var(--border)" strokeDasharray="3 3" />
                        <text x={34} y={y + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                          {v.toFixed(1)}%
                        </text>
                      </g>
                    );
                  })}
                  <polyline
                    fill="none"
                    stroke="var(--chart-indigo)"
                    strokeWidth={2.2}
                    points={shownSeries.map((p, i) => `${toX(i).toFixed(1)},${toY(p.cvPct).toFixed(1)}`).join(" ")}
                  />
                  {shownSeries.filter((p) => p.usualCvPct != null).length > 1 && (
                    <polyline
                      fill="none"
                      stroke="var(--muted)"
                      strokeDasharray="4 3"
                      strokeWidth={1.3}
                      points={shownSeries
                        .map((p, i) =>
                          p.usualCvPct != null ? `${toX(i).toFixed(1)},${toY(p.usualCvPct).toFixed(1)}` : null
                        )
                        .filter(Boolean)
                        .join(" ")}
                    />
                  )}
                  <text x={40} y={182} fontSize={9.5} fill="var(--muted)">
                    {shownSeries[0].date}
                  </text>
                  <text x={620} y={182} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                    {shownSeries[shownSeries.length - 1].date}
                  </text>
                </svg>
              );
            })()
          )}
        </div>

        <div className="card" style={{ marginBottom: 0 }}>
          <div className="card-header">
            <div>
              <strong>HRV & Resting HR Quadrant</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                Last 4 weeks vs your 60-day normal (z-scores)
              </div>
            </div>
          </div>
          {quadrantPairs.length === 0 ? (
            <div className="muted">Needs ~3 weeks of paired HRV & resting HR nights.</div>
          ) : (
            <svg viewBox="0 0 640 200" style={{ width: "100%", height: "auto", display: "block" }}>
              <line x1={320} y1={16} x2={320} y2={176} stroke="var(--border)" />
              <line x1={40} y1={96} x2={600} y2={96} stroke="var(--border)" />
              <text x={48} y={28} fontSize={10} fill="var(--accent-primary)">
                Recovered (High HRV, Low RHR)
              </text>
              <text x={592} y={168} textAnchor="end" fontSize={10} fill="var(--amber)">
                Under strain (Low HRV, High RHR)
              </text>
              {quadrantPairs.map((p, idx) => {
                const isLast = idx === quadrantPairs.length - 1;
                const x = 320 + (p.rhrZ / 3) * 265;
                const y = 96 - (p.hrvZ / 3) * 76;
                return (
                  <circle
                    key={p.date}
                    cx={x}
                    cy={y}
                    r={isLast ? 6 : 3.5}
                    fill="var(--chart-teal)"
                    fillOpacity={isLast ? 1 : 0.2 + 0.55 * (idx / quadrantPairs.length)}
                    stroke={isLast ? "var(--text)" : "none"}
                    strokeWidth={isLast ? 1.5 : 0}
                  >
                    <title>{`${p.date}: HRV z=${p.hrvZ.toFixed(1)}, RHR z=${p.rhrZ.toFixed(1)}`}</title>
                  </circle>
                );
              })}
              <text x={320} y={192} textAnchor="middle" fontSize={9.5} fill="var(--muted)">
                Resting Heart Rate (lower ← normal → higher)
              </text>
            </svg>
          )}
        </div>
      </div>

      {/* Nightly Reading Table */}
      <div className="card" style={{ marginTop: 16 }}>
        <div className="card-header">
          <strong>Nightly HRV Readings</strong>
          <span className="muted" style={{ fontSize: 12 }}>
            Showing {Math.min(30, shownSeries.length)} most recent nights
          </span>
        </div>
        <div className="table-scroll">
          <table className="data-table">
            <thead>
              <tr>
                <th>Date</th>
                <th>Overnight</th>
                <th>5m High</th>
                <th>7d Avg</th>
                <th>Baseline Band</th>
                <th>Status</th>
                <th>Resting HR</th>
                <th>Sleep</th>
              </tr>
            </thead>
            <tbody>
              {[...shownSeries]
                .reverse()
                .slice(0, 30)
                .map((p) => {
                  const badge = formatHrvBadge(p.status);
                  return (
                    <tr key={p.date}>
                      <td>{p.date}</td>
                      <td>
                        <strong>{p.ms} ms</strong>
                      </td>
                      <td>{p.fiveMinHigh != null ? `${p.fiveMinHigh} ms` : "–"}</td>
                      <td>{Math.round(p.mean7Ms)} ms</td>
                      <td>
                        {p.bandLowMs != null && p.bandHighMs != null
                          ? `${Math.round(p.bandLowMs)}–${Math.round(p.bandHighMs)} ms`
                          : "–"}
                      </td>
                      <td>
                        <span className={`pill ${badge.cls}`} style={{ fontSize: 11, padding: "2px 8px" }}>
                          {badge.label}
                        </span>
                      </td>
                      <td>{p.restingHr != null ? `${p.restingHr} bpm` : "–"}</td>
                      <td>{formatHm(p.sleepSec)}</td>
                    </tr>
                  );
                })}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
