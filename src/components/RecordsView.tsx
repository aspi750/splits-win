import { useMemo, useState } from "react";
import { Award, Download, Flag, RefreshCw, Trophy } from "lucide-react";
import type { ProgressionPoint, RecordsPayload, RecordsProgressEvent } from "../types";

interface RecordsViewProps {
  records: RecordsPayload | null;
  loading: boolean;
  error: string | null;
  progress?: RecordsProgressEvent | null;
  onRefresh: () => void;
  onOpenActivity: (activityId: number) => void;
  hasGarminKey?: boolean;
  lifetimeBackfillCompleted?: boolean;
  backfillBusy?: boolean;
  onBackfillHistory?: () => void;
}

const STANDARD_LABELS = ["1K", "5K", "10K", "Half Marathon", "Marathon"];
const PROGRESSION_DISTANCES = [
  { label: "1K", short: "1 km" },
  { label: "5K", short: "5 km" },
  { label: "10K", short: "10 km" },
  { label: "Half Marathon", short: "Half" },
  { label: "Marathon", short: "Marathon" },
] as const;

function parseDateMs(iso: string): number {
  const d = iso.slice(0, 10);
  const [y, m, day] = d.split("-").map(Number);
  return Date.UTC(y || 2020, (m || 1) - 1, day || 1);
}

function formatDuration(sec: number | null | undefined): string {
  if (!sec || !Number.isFinite(sec) || sec <= 0) return "–";
  const total = Math.round(sec);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}

function formatPace(secPerKm: number | null | undefined): string {
  if (!secPerKm || !Number.isFinite(secPerKm) || secPerKm <= 0) return "–";
  const total = Math.round(secPerKm);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}/km`;
}

export function RecordsView({
  records,
  loading,
  error,
  progress = null,
  onRefresh,
  onOpenActivity,
  hasGarminKey = false,
  lifetimeBackfillCompleted = true,
  backfillBusy = false,
  onBackfillHistory,
}: RecordsViewProps) {
  const [selectedDistance, setSelectedDistance] = useState<string>("5K");
  const [hoveredPointIdx, setHoveredPointIdx] = useState<number | null>(null);

  const progressionSeries = useMemo(() => {
    if (!records?.progression?.length) return null;
    return (
      records.progression.find((s) => s.label === selectedDistance) ??
      records.progression[0] ??
      null
    );
  }, [records, selectedDistance]);

  const pct =
    progress && progress.total > 0
      ? Math.min(100, Math.max(4, Math.round((progress.processed / progress.total) * 100)))
      : null;

  if (loading && !records) {
    return (
      <div className="card records-progress-card">
        <div className="card-header" style={{ marginBottom: 8 }}>
          <h3 style={{ margin: 0 }}>Records</h3>
          <span className="muted" style={{ fontSize: 12, fontVariantNumeric: "tabular-nums" }}>
            {progress && progress.total > 0 ? `${progress.processed} / ${progress.total}` : "Preparing…"}
          </span>
        </div>
        <div className="muted" style={{ fontSize: 13, marginBottom: 10 }}>
          {progress?.message || "Scanning your synced activities and cached FIT tracks…"}
        </div>
        <div className="progress-track">
          <div
            className={`progress-fill ${pct == null ? "indeterminate" : ""}`}
            style={pct != null ? { width: `${pct}%` } : undefined}
          />
        </div>
      </div>
    );
  }

  if (error && !records) {
    return (
      <div className="card">
        <h3 style={{ marginTop: 0 }}>Records</h3>
        <div className="key-status-err" style={{ marginBottom: 12 }}>
          {error}
        </div>
        <button type="button" className="primary" onClick={onRefresh}>
          Retry
        </button>
      </div>
    );
  }

  if (!records || (records.total_outdoor_runs === 0 && records.milestones.length === 0)) {
    return (
      <div className="card">
        <div className="card-header" style={{ marginBottom: 8 }}>
          <h3 style={{ margin: 0 }}>Records</h3>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            {hasGarminKey && !lifetimeBackfillCompleted && onBackfillHistory && (
              <button
                type="button"
                className="primary"
                disabled={backfillBusy}
                onClick={onBackfillHistory}
                style={{ display: "inline-flex", alignItems: "center", gap: 6 }}
              >
                <Download size={14} />
                {backfillBusy ? "Backfilling History…" : "Full History Backfill"}
              </button>
            )}
            <button
              type="button"
              className="secondary"
              disabled={loading}
              onClick={onRefresh}
              style={{ display: "inline-flex", alignItems: "center", gap: 6 }}
            >
              <RefreshCw size={14} />
              {loading ? "Refreshing…" : "Recompute"}
            </button>
          </div>
        </div>
        <div className="muted">
          No outdoor runs or milestones synced yet. Click <strong>Sync Watch</strong> to compute your personal records.
        </div>
      </div>
    );
  }

  const byLabel = new Map(records.distance_records.map((r) => [r.label, r]));

  return (
    <div>
      <div className="page-header">
        <div className="page-header-left">
          <h2 className="today-title">Records</h2>
          <span className="today-subtitle">
            {records.total_outdoor_runs} outdoor runs · {records.scanned_fit_runs} with 1Hz FIT best-stretch analysis
          </span>
        </div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          {hasGarminKey && !lifetimeBackfillCompleted && onBackfillHistory && (
            <button
              type="button"
              className="primary"
              disabled={backfillBusy}
              onClick={onBackfillHistory}
              style={{ display: "inline-flex", alignItems: "center", gap: 6 }}
            >
              <Download size={14} />
              {backfillBusy ? "Backfilling History…" : "Full History Backfill"}
            </button>
          )}
          <button
            type="button"
            className="secondary"
            disabled={loading}
            onClick={onRefresh}
            style={{ display: "inline-flex", alignItems: "center", gap: 6 }}
          >
            <RefreshCw size={14} />
            {loading ? "Refreshing…" : "Recompute"}
          </button>
        </div>
      </div>

      {loading && (
        <div className="backfill-progress-strip" style={{ marginBottom: 12 }}>
          <div style={{ display: "flex", justifyContent: "space-between", fontSize: 12, marginBottom: 5 }}>
            <span className="muted">
              {progress?.message || "Recomputing personal records…"}
            </span>
            <span className="muted" style={{ fontVariantNumeric: "tabular-nums" }}>
              {progress && progress.total > 0 ? `${progress.processed} / ${progress.total}` : ""}
            </span>
          </div>
          <div className="progress-track">
            <div
              className={`progress-fill ${pct == null ? "indeterminate" : ""}`}
              style={pct != null ? { width: `${pct}%` } : undefined}
            />
          </div>
        </div>
      )}

      {error && (
        <div
          className="card"
          style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 10, marginBottom: 12 }}
        >
          <span className="key-status-err">{error}</span>
          <button type="button" className="secondary" onClick={onRefresh}>
            Retry
          </button>
        </div>
      )}

      {/* Standard Distance Personal Records */}
      <div className="card">
        <div className="card-header">
          <div>
            <strong>Personal Records</strong>
            <div className="muted" style={{ fontSize: 12 }}>
              Fastest stretches from cached 1Hz FIT tracks or full outdoor run averages · Click any record to inspect
            </div>
          </div>
        </div>
        <div className="record-grid">
          {STANDARD_LABELS.map((label) => {
            const entry = byLabel.get(label);
            return (
              <button
                key={label}
                type="button"
                className="record-tile"
                disabled={!entry}
                onClick={() => entry && onOpenActivity(entry.activity_id)}
              >
                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", width: "100%" }}>
                  <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12.5, color: "var(--muted)", fontWeight: 600 }}>
                    <Trophy size={14} color="var(--accent-primary)" />
                    {label}
                  </span>
                  {entry && (
                    <span className="pill green" style={{ fontSize: 10.5, padding: "1px 7px" }}>
                      {entry.source === "fit_best_stretch" ? "1Hz FIT" : "Run Avg"}
                    </span>
                  )}
                </div>
                <div style={{ fontSize: 25, fontWeight: 700, fontVariantNumeric: "tabular-nums" }}>
                  {entry ? formatDuration(entry.time_sec) : "–"}
                </div>
                {entry ? (
                  <>
                    <div style={{ fontSize: 13, fontWeight: 600, color: "var(--accent-primary)" }}>
                      {formatPace(entry.pace_sec_per_km)}
                      {entry.average_hr ? ` · ${Math.round(entry.average_hr)} bpm` : ""}
                    </div>
                    <div className="muted" style={{ fontSize: 12 }}>
                      {entry.activity_name} · {entry.date}
                    </div>
                  </>
                ) : (
                  <div className="muted" style={{ fontSize: 12 }}>
                    No outdoor run this distance yet
                  </div>
                )}
              </button>
            );
          })}
        </div>
      </div>

      {/* Race Predictions — "If you raced today" */}
      {records.distance_records.length > 0 && (
        <div className="card">
          <div className="card-header">
            <div>
              <strong>If you raced today</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                Predicted from your best efforts of the last {records.prediction_window_months} months
              </div>
            </div>
          </div>
          {records.predictions.length === 0 ? (
            <div className="muted" style={{ fontSize: 13 }}>
              Needs a run of 5 km or more in the last year.
            </div>
          ) : (
            <>
              <div className="record-grid">
                {records.predictions.map((p) => (
                  <button
                    key={p.label}
                    type="button"
                    className="record-tile"
                    onClick={() => onOpenActivity(p.source_activity_id)}
                  >
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12.5, color: "var(--muted)", fontWeight: 600 }}>
                      <Flag size={14} color="var(--accent-primary)" />
                      {p.label}
                    </span>
                    <div style={{ fontSize: 23, fontWeight: 700, fontVariantNumeric: "tabular-nums" }}>
                      {formatDuration(p.predicted_sec)}
                    </div>
                    <div style={{ fontSize: 13, fontWeight: 600, color: "var(--accent-primary)" }}>
                      {formatPace(p.pace_sec_per_km)}
                    </div>
                    <div className="muted" style={{ fontSize: 12 }}>
                      From your {p.source_label} of {formatDuration(p.source_time_sec)}, {p.source_date}
                    </div>
                  </button>
                ))}
              </div>
              <div className="muted" style={{ fontSize: 11.5, marginTop: 10 }}>
                Riegel&apos;s formula on the fastest stretches inside your runs. Training efforts are rarely all-out, so on a rested race day you&apos;ll often beat these. The marathon also needs the long runs to back it up.
              </div>
            </>
          )}
        </div>
      )}

      {/* Progression Chart — "How your record came down" */}
      {records.distance_records.length > 0 && (
        <div className="card">
          <div className="card-header">
            <div>
              <strong>How your record came down</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                {(() => {
                  const shortName =
                    PROGRESSION_DISTANCES.find((d) => d.label === selectedDistance)?.short ??
                    selectedDistance;
                  if (!progressionSeries || progressionSeries.total_runs === 0) {
                    return `No ${shortName} efforts yet`;
                  }
                  const brokenLabel =
                    progressionSeries.broken_count === 1 ? "time" : "times";
                  return `${progressionSeries.total_runs} runs with a ${shortName} in them · record broken ${progressionSeries.broken_count} ${brokenLabel}`;
                })()}
              </div>
            </div>
            <div className="segmented-control" role="tablist" aria-label="Progression distance">
              {PROGRESSION_DISTANCES.map((d) => (
                <button
                  key={d.label}
                  type="button"
                  className={`seg-btn ${selectedDistance === d.label ? "active" : ""}`}
                  onClick={() => {
                    setSelectedDistance(d.label);
                    setHoveredPointIdx(null);
                  }}
                >
                  {d.short}
                </button>
              ))}
            </div>
          </div>

          {!progressionSeries || progressionSeries.points.length === 0 ? (
            <div className="muted" style={{ fontSize: 13 }}>
              No recorded outdoor efforts at this distance yet.
            </div>
          ) : (
            (() => {
              const pts: ProgressionPoint[] = progressionSeries.points;
              const W = 860;
              const H = 240;
              const padLeft = 58;
              const padRight = 18;
              const padTop = 16;
              const padBottom = 28;
              const plotW = W - padLeft - padRight;
              const plotH = H - padTop - padBottom;

              const dateValues = pts.map((p) => parseDateMs(p.date));
              const nowMs = Date.now();
              const rawMinMs = Math.min(...dateValues);
              const rawMaxMs = Math.max(nowMs, ...dateValues);
              const minMs = rawMaxMs > rawMinMs ? rawMinMs : rawMinMs - 86_400_000;
              const maxMs = rawMaxMs > rawMinMs ? rawMaxMs : rawMaxMs + 86_400_000;

              const timeValues = pts.map((p) => p.time_sec);
              const fastest = Math.min(...timeValues);
              const slowest = Math.max(...timeValues);
              const minSec = fastest * 0.98;
              const maxSec = slowest > fastest ? slowest * 1.02 : fastest * 1.05 + 10;

              const xForMs = (ms: number) =>
                padLeft + ((ms - minMs) / Math.max(1, maxMs - minMs)) * plotW;
              // Faster times sit higher on the chart (matching macOS -e.seconds).
              const yForSec = (sec: number) =>
                padTop + ((sec - minSec) / Math.max(1, maxSec - minSec)) * plotH;

              const yTicks = [0, 0.33, 0.67, 1].map((frac) => minSec + frac * (maxSec - minSec));

              const breaks = pts.filter((p) => p.is_record_break);
              let stepPath = "";
              if (breaks.length > 0) {
                const firstX = xForMs(parseDateMs(breaks[0].date));
                const firstY = yForSec(breaks[0].time_sec);
                stepPath = `M ${firstX.toFixed(1)} ${firstY.toFixed(1)}`;
                for (let i = 1; i < breaks.length; i++) {
                  const bx = xForMs(parseDateMs(breaks[i].date));
                  const by = yForSec(breaks[i].time_sec);
                  stepPath += ` H ${bx.toFixed(1)} V ${by.toFixed(1)}`;
                }
                stepPath += ` H ${xForMs(nowMs).toFixed(1)}`;
              }

              const hoveredPt = hoveredPointIdx !== null ? pts[hoveredPointIdx] : null;
              const startLabel = pts[0].date;
              const midMs = (minMs + maxMs) / 2;
              const midLabel = new Date(midMs).toISOString().slice(0, 10);
              const endLabel = new Date(nowMs).toISOString().slice(0, 10);

              return (
                <div className="svg-chart-wrap" onMouseLeave={() => setHoveredPointIdx(null)}>
                  <svg viewBox={`0 0 ${W} ${H}`} style={{ width: "100%", height: "auto", display: "block" }}>
                    {yTicks.map((t, i) => {
                      const y = yForSec(t);
                      return (
                        <g key={i}>
                          <line
                            x1={padLeft}
                            y1={y}
                            x2={W - padRight}
                            y2={y}
                            stroke="var(--border)"
                            strokeDasharray="3 3"
                          />
                          <text
                            x={padLeft - 8}
                            y={y + 3.5}
                            textAnchor="end"
                            fontSize={10}
                            fill="var(--muted)"
                            style={{ fontVariantNumeric: "tabular-nums" }}
                          >
                            {formatDuration(t)}
                          </text>
                        </g>
                      );
                    })}

                    <text x={padLeft} y={H - 6} fontSize={10} fill="var(--muted)">
                      {startLabel}
                    </text>
                    {maxMs - minMs > 45 * 86_400_000 && (
                      <text
                        x={padLeft + plotW / 2}
                        y={H - 6}
                        textAnchor="middle"
                        fontSize={10}
                        fill="var(--muted)"
                      >
                        {midLabel}
                      </text>
                    )}
                    <text x={W - padRight} y={H - 6} textAnchor="end" fontSize={10} fill="var(--muted)">
                      {endLabel}
                    </text>

                    {pts.map((p, idx) => {
                      const cx = xForMs(parseDateMs(p.date));
                      const cy = yForSec(p.time_sec);
                      const isHovered = hoveredPointIdx === idx;
                      return (
                        <g key={`${p.activity_id}-${idx}`}>
                          {!p.is_record_break && (
                            <circle
                              cx={cx}
                              cy={cy}
                              r={isHovered ? 5 : 3.2}
                              fill="var(--accent-primary)"
                              fillOpacity={isHovered ? 0.9 : 0.32}
                            />
                          )}
                          <circle
                            cx={cx}
                            cy={cy}
                            r={9}
                            fill="transparent"
                            style={{ cursor: "pointer" }}
                            onMouseEnter={() => setHoveredPointIdx(idx)}
                            onClick={() => onOpenActivity(p.activity_id)}
                          />
                        </g>
                      );
                    })}

                    {stepPath && (
                      <path
                        d={stepPath}
                        fill="none"
                        stroke="var(--accent-primary)"
                        strokeWidth={2.5}
                        strokeLinejoin="round"
                        pointerEvents="none"
                      />
                    )}

                    {pts.map((p, idx) => {
                      if (!p.is_record_break) return null;
                      const cx = xForMs(parseDateMs(p.date));
                      const cy = yForSec(p.time_sec);
                      const isHovered = hoveredPointIdx === idx;
                      return (
                        <circle
                          key={`brk-${p.activity_id}-${idx}`}
                          cx={cx}
                          cy={cy}
                          r={isHovered ? 6.5 : 4.8}
                          fill="var(--accent-primary)"
                          stroke="var(--card)"
                          strokeWidth={1.6}
                          style={{ cursor: "pointer" }}
                          onMouseEnter={() => setHoveredPointIdx(idx)}
                          onClick={() => onOpenActivity(p.activity_id)}
                        />
                      );
                    })}
                  </svg>

                  {hoveredPt && (
                    <div className="chart-tooltip" style={{ top: 8, right: 8 }}>
                      <strong>
                        {hoveredPt.activity_name}
                        {hoveredPt.is_record_break ? " ★ Record" : ""}
                      </strong>
                      <div>
                        {hoveredPt.date} · {formatDuration(hoveredPt.time_sec)} (
                        {formatPace(hoveredPt.pace_sec_per_km)})
                      </div>
                    </div>
                  )}
                </div>
              );
            })()
          )}
        </div>
      )}

      {/* Volume & Elevation Milestones */}
      {records.milestones.length > 0 && (
        <div className="card">
          <div className="card-header">
            <div>
              <strong>Volume &amp; Elevation Milestones</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                All-time distance, climbing, weekly volume, duration, and training load highs
              </div>
            </div>
          </div>
          <div className="record-grid">
            {records.milestones.map((m) => (
              <button
                key={m.key}
                type="button"
                className="record-tile"
                disabled={m.activity_id == null}
                onClick={() => m.activity_id != null && onOpenActivity(m.activity_id)}
              >
                <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12.5, color: "var(--muted)", fontWeight: 600 }}>
                  <Award size={14} color="var(--amber)" />
                  {m.label}
                </span>
                <div style={{ fontSize: 23, fontWeight: 700, fontVariantNumeric: "tabular-nums" }}>
                  {m.value_display}
                </div>
                <div className="muted" style={{ fontSize: 12 }}>
                  {m.activity_name ? `${m.activity_name} · ` : ""}
                  {m.date}
                  {m.pace_sec_per_km ? ` · ${formatPace(m.pace_sec_per_km)}` : ""}
                  {m.average_hr ? ` · ${Math.round(m.average_hr)} bpm` : ""}
                </div>
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Best of Each Year Table */}
      {records.yearly_bests.length > 0 && (
        <div className="card">
          <div className="card-header">
            <div>
              <strong>Best of Each Year</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                Click any time to open the underlying workout
              </div>
            </div>
          </div>
          <div className="table-scroll">
            <table className="data-table">
              <thead>
                <tr>
                  <th>Year</th>
                  {STANDARD_LABELS.map((lbl) => (
                    <th key={lbl}>{lbl}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {records.yearly_bests.map((yr) => (
                  <tr key={yr.year}>
                    <td>
                      <strong>{yr.year}</strong>
                    </td>
                    {STANDARD_LABELS.map((lbl) => {
                      const cell = yr.bests[lbl];
                      const allTime = byLabel.get(lbl);
                      const isPr =
                        cell &&
                        allTime &&
                        cell.activity_id === allTime.activity_id &&
                        Math.abs(cell.time_sec - allTime.time_sec) < 0.5;
                      return (
                        <td key={lbl}>
                          {cell ? (
                            <button
                              type="button"
                              className="secondary"
                              style={{
                                padding: "3px 8px",
                                fontSize: 12.5,
                                fontWeight: isPr ? 700 : 500,
                                borderColor: isPr ? "var(--accent-border)" : "transparent",
                                background: isPr ? "var(--accent-soft-bg)" : "transparent",
                                color: isPr ? "var(--accent-primary)" : "var(--text)",
                              }}
                              title={`${cell.activity_name} (${cell.date}) · ${formatPace(cell.pace_sec_per_km)}`}
                              onClick={() => onOpenActivity(cell.activity_id)}
                            >
                              {formatDuration(cell.time_sec)} {isPr ? "★" : ""}
                            </button>
                          ) : (
                            <span className="muted">–</span>
                          )}
                        </td>
                      );
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Garmin PR Flagged Activities */}
      {records.pr_activities.length > 0 && (
        <div className="card">
          <div className="card-header">
            <strong>Garmin PR-Flagged Workouts ({records.pr_activities.length})</strong>
          </div>
          <div className="table-scroll">
            <table className="data-table">
              <thead>
                <tr>
                  <th>Date</th>
                  <th>Activity</th>
                  <th>Distance</th>
                  <th>Duration</th>
                  <th>Avg HR</th>
                </tr>
              </thead>
              <tbody>
                {records.pr_activities.map((a) => (
                  <tr
                    key={a.garmin_id}
                    className="clickable-row"
                    onClick={() => onOpenActivity(a.garmin_id)}
                  >
                    <td>{a.start_iso.slice(0, 10)}</td>
                    <td>
                      <strong>{a.name || a.type_key}</strong>
                    </td>
                    <td>{(a.distance_m / 1000).toFixed(2)} km</td>
                    <td>{formatDuration(a.moving_duration_sec > 0 ? a.moving_duration_sec : a.duration_sec)}</td>
                    <td>{a.average_hr ? `${Math.round(a.average_hr)} bpm` : "–"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}
