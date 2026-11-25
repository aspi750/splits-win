import { useMemo, useState } from "react";
import { RotateCcw, Search } from "lucide-react";
import type { ActivityRecord } from "../types";

interface ExplorerViewProps {
  activities: ActivityRecord[];
  loading: boolean;
  onOpenActivity: (activityId: number) => void;
}

type DatePreset = "all" | "30d" | "90d" | "1y" | "ytd" | "custom";
type PlotMode = "pace_hr" | "distance_date" | "load_date";
type SortField = "date" | "name" | "sport" | "distance" | "duration" | "pace" | "hr" | "elev" | "load";

function formatDuration(sec: number): string {
  const total = Math.max(0, Math.round(sec));
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

function activeSec(a: ActivityRecord): number {
  return a.moving_duration_sec > 0 ? a.moving_duration_sec : a.duration_sec;
}

function activityPaceSecPerKm(a: ActivityRecord): number | null {
  const sec = activeSec(a);
  if (a.distance_m <= 100 || sec <= 0) return null;
  const p = sec / (a.distance_m / 1000);
  return p > 120 && p < 1200 ? p : null;
}

function toIsoDate(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")}`;
}

export function ExplorerView({ activities, loading, onOpenActivity }: ExplorerViewProps) {
  const [query, setQuery] = useState("");
  const [sportFilter, setSportFilter] = useState("all");
  const [shoeFilter, setShoeFilter] = useState("all");
  const [datePreset, setDatePreset] = useState<DatePreset>("all");
  const [fromDate, setFromDate] = useState("");
  const [toDate, setToDate] = useState("");
  const [minDistKm, setMinDistKm] = useState("");
  const [maxDistKm, setMaxDistKm] = useState("");
  const [minHr, setMinHr] = useState("");
  const [maxHr, setMaxHr] = useState("");
  const [hyroxOnly, setHyroxOnly] = useState(false);

  const [plotMode, setPlotMode] = useState<PlotMode>("pace_hr");
  const [sortField, setSortField] = useState<SortField>("date");
  const [sortAsc, setSortAsc] = useState(false);
  const [showAllRows, setShowAllRows] = useState(false);

  const sportOptions = useMemo(() => {
    const counts = new Map<string, number>();
    for (const a of activities) {
      counts.set(a.type_key, (counts.get(a.type_key) ?? 0) + 1);
    }
    return Array.from(counts.entries()).sort((a, b) => b[1] - a[1]);
  }, [activities]);

  const shoeOptions = useMemo(() => {
    const set = new Set<string>();
    for (const a of activities) {
      if (a.shoe_name && a.shoe_name.trim()) set.add(a.shoe_name.trim());
    }
    return Array.from(set).sort();
  }, [activities]);

  const effectiveDateBounds = useMemo(() => {
    const now = new Date();
    if (datePreset === "all") return { from: "", to: "" };
    if (datePreset === "custom") return { from: fromDate, to: toDate };
    if (datePreset === "ytd") return { from: `${now.getFullYear()}-01-01`, to: "" };
    const daysBack = datePreset === "30d" ? 30 : datePreset === "90d" ? 90 : 365;
    const d = new Date(now);
    d.setDate(now.getDate() - daysBack);
    return { from: toIsoDate(d), to: "" };
  }, [datePreset, fromDate, toDate]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const minD = minDistKm.trim() ? Number(minDistKm) * 1000 : null;
    const maxD = maxDistKm.trim() ? Number(maxDistKm) * 1000 : null;
    const minH = minHr.trim() ? Number(minHr) : null;
    const maxH = maxHr.trim() ? Number(maxHr) : null;

    return activities.filter((a) => {
      if (sportFilter !== "all" && a.type_key !== sportFilter) return false;
      if (shoeFilter !== "all" && (a.shoe_name ?? "").trim() !== shoeFilter) return false;
      if (hyroxOnly && a.is_hyrox !== true) return false;

      const dStr = a.start_iso.slice(0, 10);
      if (effectiveDateBounds.from && dStr < effectiveDateBounds.from) return false;
      if (effectiveDateBounds.to && dStr > effectiveDateBounds.to) return false;

      if (minD != null && Number.isFinite(minD) && a.distance_m < minD) return false;
      if (maxD != null && Number.isFinite(maxD) && a.distance_m > maxD) return false;

      if (minH != null && Number.isFinite(minH) && (a.average_hr == null || a.average_hr < minH)) {
        return false;
      }
      if (maxH != null && Number.isFinite(maxH) && (a.average_hr == null || a.average_hr > maxH)) {
        return false;
      }

      if (q) {
        const hay = `${a.name} ${a.type_key} ${a.shoe_name ?? ""} ${a.garmin_notes ?? ""}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [
    activities,
    query,
    sportFilter,
    shoeFilter,
    hyroxOnly,
    effectiveDateBounds,
    minDistKm,
    maxDistKm,
    minHr,
    maxHr,
  ]);

  const sorted = useMemo(() => {
    const list = [...filtered];
    list.sort((a, b) => {
      let cmp = 0;
      switch (sortField) {
        case "date":
          cmp = a.start_iso.localeCompare(b.start_iso);
          break;
        case "name":
          cmp = (a.name || a.type_key).localeCompare(b.name || b.type_key);
          break;
        case "sport":
          cmp = a.type_key.localeCompare(b.type_key);
          break;
        case "distance":
          cmp = a.distance_m - b.distance_m;
          break;
        case "duration":
          cmp = activeSec(a) - activeSec(b);
          break;
        case "pace":
          cmp = (activityPaceSecPerKm(a) ?? 9999) - (activityPaceSecPerKm(b) ?? 9999);
          break;
        case "hr":
          cmp = (a.average_hr ?? 0) - (b.average_hr ?? 0);
          break;
        case "elev":
          cmp = a.elevation_gain_m - b.elevation_gain_m;
          break;
        case "load":
          cmp = (a.training_load ?? 0) - (b.training_load ?? 0);
          break;
      }
      return sortAsc ? cmp : -cmp;
    });
    return list;
  }, [filtered, sortField, sortAsc]);

  const sliceTotals = useMemo(() => {
    const km = filtered.reduce((s, a) => s + a.distance_m / 1000, 0);
    const hours = filtered.reduce((s, a) => s + activeSec(a), 0) / 3600;
    const elev = filtered.reduce((s, a) => s + Math.max(0, a.elevation_gain_m), 0);
    const load = filtered.reduce((s, a) => s + (a.training_load ?? 0), 0);
    const hrs = filtered.map((a) => a.average_hr).filter((v): v is number => v != null && v > 0);
    const avgHr = hrs.length ? Math.round(hrs.reduce((a, b) => a + b, 0) / hrs.length) : null;
    const paces = filtered
      .map(activityPaceSecPerKm)
      .filter((v): v is number => v != null);
    const avgPace = paces.length ? paces.reduce((a, b) => a + b, 0) / paces.length : null;
    return { km, hours, elev, load, avgHr, avgPace };
  }, [filtered]);

  function toggleSort(field: SortField) {
    if (sortField === field) {
      setSortAsc(!sortAsc);
    } else {
      setSortField(field);
      setSortAsc(false);
    }
  }

  function resetFilters() {
    setQuery("");
    setSportFilter("all");
    setShoeFilter("all");
    setDatePreset("all");
    setFromDate("");
    setToDate("");
    setMinDistKm("");
    setMaxDistKm("");
    setMinHr("");
    setMaxHr("");
    setHyroxOnly(false);
  }

  if (loading && activities.length === 0) {
    return (
      <div className="card">
        <h3 style={{ marginTop: 0 }}>Explorer</h3>
        <div className="muted">Loading full activity dataset…</div>
      </div>
    );
  }

  if (activities.length === 0) {
    return (
      <div className="card">
        <h3 style={{ marginTop: 0 }}>Explorer</h3>
        <div className="muted">
          No activities synced yet. Use <strong>Sync Watch</strong> in the sidebar to explore your training history.
        </div>
      </div>
    );
  }

  const displayedRows = showAllRows ? sorted : sorted.slice(0, 50);

  return (
    <div>
      <div className="page-header">
        <div className="page-header-left">
          <h2 className="today-title">Explorer</h2>
          <span className="today-subtitle">
            Filter, scatter-plot, and inspect {activities.length.toLocaleString()} synced workouts
          </span>
        </div>
        <button
          type="button"
          className="secondary"
          onClick={resetFilters}
          style={{ display: "inline-flex", alignItems: "center", gap: 6 }}
        >
          <RotateCcw size={14} /> Reset Filters
        </button>
      </div>

      {/* Multi-Filter Bar */}
      <div className="card">
        <div className="filter-bar">
          <div style={{ gridColumn: "span 2" }}>
            <label className="muted" style={{ fontSize: 11.5, display: "block", marginBottom: 4 }}>
              Keyword Search
            </label>
            <div style={{ position: "relative" }}>
              <Search
                size={14}
                style={{ position: "absolute", left: 10, top: "50%", transform: "translateY(-50%)", color: "var(--muted)" }}
              />
              <input
                style={{ paddingLeft: 30 }}
                placeholder="Search title, sport, shoe, or notes…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </div>
          </div>

          <div>
            <label className="muted" style={{ fontSize: 11.5, display: "block", marginBottom: 4 }}>
              Sport
            </label>
            <select value={sportFilter} onChange={(e) => setSportFilter(e.target.value)}>
              <option value="all">All Sports ({activities.length})</option>
              {sportOptions.map(([key, count]) => (
                <option key={key} value={key}>
                  {key} ({count})
                </option>
              ))}
            </select>
          </div>

          <div>
            <label className="muted" style={{ fontSize: 11.5, display: "block", marginBottom: 4 }}>
              Date Range
            </label>
            <select value={datePreset} onChange={(e) => setDatePreset(e.target.value as DatePreset)}>
              <option value="all">All Time</option>
              <option value="30d">Last 30 Days</option>
              <option value="90d">Last 90 Days</option>
              <option value="1y">Last 1 Year</option>
              <option value="ytd">Year to Date</option>
              <option value="custom">Custom Range</option>
            </select>
          </div>

          <div>
            <label className="muted" style={{ fontSize: 11.5, display: "block", marginBottom: 4 }}>
              Min / Max Dist (km)
            </label>
            <div style={{ display: "flex", gap: 6 }}>
              <input
                type="number"
                min="0"
                step="0.5"
                placeholder="Min"
                value={minDistKm}
                onChange={(e) => setMinDistKm(e.target.value)}
              />
              <input
                type="number"
                min="0"
                step="0.5"
                placeholder="Max"
                value={maxDistKm}
                onChange={(e) => setMaxDistKm(e.target.value)}
              />
            </div>
          </div>

          <div>
            <label className="muted" style={{ fontSize: 11.5, display: "block", marginBottom: 4 }}>
              Min / Max Avg HR
            </label>
            <div style={{ display: "flex", gap: 6 }}>
              <input
                type="number"
                min="40"
                max="220"
                placeholder="Min"
                value={minHr}
                onChange={(e) => setMinHr(e.target.value)}
              />
              <input
                type="number"
                min="40"
                max="220"
                placeholder="Max"
                value={maxHr}
                onChange={(e) => setMaxHr(e.target.value)}
              />
            </div>
          </div>

          <div>
            <label className="muted" style={{ fontSize: 11.5, display: "block", marginBottom: 4 }}>
              Shoe
            </label>
            <select value={shoeFilter} onChange={(e) => setShoeFilter(e.target.value)}>
              <option value="all">All Shoes</option>
              {shoeOptions.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </select>
          </div>

          <div style={{ display: "flex", alignItems: "center", gap: 8, paddingBottom: 6 }}>
            <input
              id="explorer-hyrox-toggle"
              type="checkbox"
              checked={hyroxOnly}
              onChange={(e) => setHyroxOnly(e.target.checked)}
              style={{ width: "auto" }}
            />
            <label htmlFor="explorer-hyrox-toggle" style={{ fontSize: 13, cursor: "pointer" }}>
              HYROX Only
            </label>
          </div>
        </div>

        {datePreset === "custom" && (
          <div style={{ display: "flex", gap: 10, marginTop: 10, flexWrap: "wrap" }}>
            <div style={{ width: 170 }}>
              <label className="muted" style={{ fontSize: 11.5 }}>From</label>
              <input type="date" value={fromDate} onChange={(e) => setFromDate(e.target.value)} />
            </div>
            <div style={{ width: 170 }}>
              <label className="muted" style={{ fontSize: 11.5 }}>To</label>
              <input type="date" value={toDate} onChange={(e) => setToDate(e.target.value)} />
            </div>
          </div>
        )}
      </div>

      {/* Filtered Slice Summary KPIs */}
      <div className="kpi-grid">
        <div className="kpi-tile">
          <span className="kpi-tile-label">Matching</span>
          <span className="kpi-tile-val">{filtered.length.toLocaleString()}</span>
          <span className="kpi-tile-note">workouts</span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Distance</span>
          <span className="kpi-tile-val">{sliceTotals.km.toFixed(1)} km</span>
          <span className="kpi-tile-note">filtered total</span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Moving Time</span>
          <span className="kpi-tile-val">{sliceTotals.hours.toFixed(1)} h</span>
          <span className="kpi-tile-note">active duration</span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Elevation</span>
          <span className="kpi-tile-val">{Math.round(sliceTotals.elev).toLocaleString()} m</span>
          <span className="kpi-tile-note">total climb</span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Avg Pace / HR</span>
          <span className="kpi-tile-val" style={{ fontSize: 20 }}>
            {formatPace(sliceTotals.avgPace)}
          </span>
          <span className="kpi-tile-note">
            {sliceTotals.avgHr != null ? `${sliceTotals.avgHr} bpm avg` : "–"}
          </span>
        </div>
      </div>

      {/* Interactive Scatter Plot */}
      <div className="card">
        <div className="card-header">
          <div>
            <strong>Interactive Scatter Plot</strong>
            <div className="muted" style={{ fontSize: 12 }}>
              Click any point to inspect that workout&apos;s Activity Detail
            </div>
          </div>
          <div className="segmented-control">
            <button
              type="button"
              className={`seg-btn ${plotMode === "pace_hr" ? "active" : ""}`}
              onClick={() => setPlotMode("pace_hr")}
            >
              Pace vs. HR
            </button>
            <button
              type="button"
              className={`seg-btn ${plotMode === "distance_date" ? "active" : ""}`}
              onClick={() => setPlotMode("distance_date")}
            >
              Distance vs. Date
            </button>
            <button
              type="button"
              className={`seg-btn ${plotMode === "load_date" ? "active" : ""}`}
              onClick={() => setPlotMode("load_date")}
            >
              Load vs. Date
            </button>
          </div>
        </div>

        {(() => {
          const pts = filtered
            .map((a) => {
              const ms = new Date(a.start_iso.slice(0, 10)).getTime();
              const pace = activityPaceSecPerKm(a);
              if (plotMode === "pace_hr") {
                if (pace == null || a.average_hr == null || a.average_hr <= 0) return null;
                return { a, x: a.average_hr, y: pace };
              }
              if (plotMode === "distance_date") {
                if (a.distance_m <= 0 || Number.isNaN(ms)) return null;
                return { a, x: ms, y: a.distance_m / 1000 };
              }
              if (a.training_load == null || a.training_load <= 0 || Number.isNaN(ms)) return null;
              return { a, x: ms, y: a.training_load };
            })
            .filter((p): p is { a: ActivityRecord; x: number; y: number } => p != null);

          if (pts.length < 2) {
            return <div className="muted">Not enough matching workouts with required metrics to plot.</div>;
          }

          const xs = pts.map((p) => p.x);
          const ys = pts.map((p) => p.y);
          const minX = Math.min(...xs);
          const maxX = Math.max(...xs);
          const spanX = Math.max(1, maxX - minX);
          const minY = Math.min(...ys);
          const maxY = Math.max(...ys);
          const spanY = Math.max(1, maxY - minY);
          const toSvgX = (x: number) => 56 + ((x - minX) / spanX) * 790;
          // For Pace vs HR, invert Y so faster (lower sec/km) sits higher
          const toSvgY = (y: number) =>
            plotMode === "pace_hr"
              ? 20 + ((y - minY) / spanY) * 165
              : 185 - ((y - minY) / spanY) * 165;

          return (
            <svg viewBox="0 0 880 212" style={{ width: "100%", height: "auto", display: "block" }}>
              {[0, 0.5, 1].map((f) => {
                const yVal = minY + f * spanY;
                const sy = toSvgY(yVal);
                const label =
                  plotMode === "pace_hr"
                    ? formatPace(yVal).replace("/km", "")
                    : plotMode === "distance_date"
                    ? `${yVal.toFixed(1)}k`
                    : `${Math.round(yVal)}`;
                return (
                  <g key={f}>
                    <line x1={56} y1={sy} x2={846} y2={sy} stroke="var(--border)" strokeDasharray="3 3" />
                    <text x={50} y={sy + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                      {label}
                    </text>
                  </g>
                );
              })}
              {pts.slice(0, 600).map((p) => (
                <circle
                  key={p.a.garmin_id}
                  cx={toSvgX(p.x)}
                  cy={toSvgY(p.y)}
                  r={4.2}
                  fill={p.a.is_hyrox ? "var(--amber)" : "var(--accent-primary)"}
                  fillOpacity={0.65}
                  style={{ cursor: "pointer" }}
                  onClick={() => onOpenActivity(p.a.garmin_id)}
                >
                  <title>
                    {`${p.a.start_iso.slice(0, 10)} · ${p.a.name || p.a.type_key} · ${(
                      p.a.distance_m / 1000
                    ).toFixed(2)} km${p.a.average_hr ? ` · ${Math.round(p.a.average_hr)} bpm` : ""}`}
                  </title>
                </circle>
              ))}
              <text x={56} y={204} fontSize={9.5} fill="var(--muted)">
                {plotMode === "pace_hr" ? `${Math.round(minX)} bpm` : toIsoDate(new Date(minX))}
              </text>
              <text x={846} y={204} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                {plotMode === "pace_hr" ? `${Math.round(maxX)} bpm` : toIsoDate(new Date(maxX))}
              </text>
            </svg>
          );
        })()}
      </div>

      {/* Sortable Results Table */}
      <div className="card">
        <div className="card-header">
          <strong>Results ({sorted.length})</strong>
          {sorted.length > 50 && (
            <button
              type="button"
              className="secondary"
              style={{ padding: "4px 10px", fontSize: 12 }}
              onClick={() => setShowAllRows((v) => !v)}
            >
              {showAllRows ? "Show top 50" : `Show all ${sorted.length}`}
            </button>
          )}
        </div>
        <div className="table-scroll">
          <table className="data-table">
            <thead>
              <tr>
                <th className="sortable" onClick={() => toggleSort("date")}>Date</th>
                <th className="sortable" onClick={() => toggleSort("name")}>Activity</th>
                <th className="sortable" onClick={() => toggleSort("sport")}>Sport</th>
                <th className="sortable" onClick={() => toggleSort("distance")}>Dist</th>
                <th className="sortable" onClick={() => toggleSort("duration")}>Time</th>
                <th className="sortable" onClick={() => toggleSort("pace")}>Pace</th>
                <th className="sortable" onClick={() => toggleSort("hr")}>Avg HR</th>
                <th className="sortable" onClick={() => toggleSort("elev")}>Elev</th>
                <th className="sortable" onClick={() => toggleSort("load")}>Load</th>
                <th>Shoe</th>
              </tr>
            </thead>
            <tbody>
              {displayedRows.map((a) => {
                const dur = activeSec(a);
                const pace = activityPaceSecPerKm(a);
                return (
                  <tr
                    key={a.garmin_id}
                    className="clickable-row"
                    onClick={() => onOpenActivity(a.garmin_id)}
                  >
                    <td>{a.start_iso.slice(0, 10)}</td>
                    <td>
                      <strong>{a.name || a.type_key}</strong>
                      {a.is_hyrox ? (
                        <span className="pill amber" style={{ marginLeft: 6, fontSize: 10, padding: "1px 6px" }}>
                          HYROX
                        </span>
                      ) : null}
                    </td>
                    <td className="muted">{a.type_key}</td>
                    <td>{(a.distance_m / 1000).toFixed(2)} km</td>
                    <td>{formatDuration(dur)}</td>
                    <td>{formatPace(pace)}</td>
                    <td>{a.average_hr ? `${Math.round(a.average_hr)} bpm` : "–"}</td>
                    <td>{a.elevation_gain_m > 0 ? `${Math.round(a.elevation_gain_m)} m` : "–"}</td>
                    <td>{a.training_load ? Math.round(a.training_load) : "–"}</td>
                    <td className="muted">{a.shoe_name || "–"}</td>
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
