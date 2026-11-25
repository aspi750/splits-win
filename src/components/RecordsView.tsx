import { Award, Download, Flag, RefreshCw, Trophy } from "lucide-react";
import type { RecordsPayload, RecordsProgressEvent } from "../types";

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

      {/* Volume & Elevation Milestones */}
      {records.milestones.length > 0 && (
        <div className="card">
          <div className="card-header">
            <div>
              <strong>Volume & Elevation Milestones</strong>
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

      {/* Race Predictions */}
      {records.predictions.length > 0 && (
        <div className="card">
          <div className="card-header">
            <div>
              <strong>If You Raced Today</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                Riegel predictions from your best efforts in the last {records.prediction_window_months} months
              </div>
            </div>
          </div>
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
                  From {p.source_label} ({formatDuration(p.source_time_sec)}) on {p.source_date}
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
