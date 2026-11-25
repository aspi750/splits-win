import { useMemo, useState } from "react";
import type { ActivityRecord, DayRecord } from "../types";

type TrendsRange = "30d" | "90d" | "1y" | "all";
type VolumeMetric = "km" | "hours";

interface TrendsViewProps {
  activities: ActivityRecord[];
  days: DayRecord[];
  loading: boolean;
  onOpenActivity: (activityId: number) => void;
}

const RANGE_OPTIONS: { id: TrendsRange; label: string; days: number | null }[] = [
  { id: "30d", label: "30D", days: 30 },
  { id: "90d", label: "90D", days: 90 },
  { id: "1y", label: "1Y", days: 365 },
  { id: "all", label: "All", days: null },
];

type SportBucket = "Run" | "Ride" | "Swim" | "Walk / Hike" | "Strength / Other";

const SPORT_COLORS: Record<SportBucket, string> = {
  Run: "var(--accent-primary)",
  Ride: "var(--chart-blue)",
  Swim: "var(--chart-teal)",
  "Walk / Hike": "var(--amber)",
  "Strength / Other": "var(--chart-purple)",
};

function classifySport(typeKey: string): SportBucket {
  const k = typeKey.toLowerCase();
  if (k.includes("run")) return "Run";
  if (k.includes("cycl") || k.includes("bik") || k.includes("ride")) return "Ride";
  if (k.includes("swim")) return "Swim";
  if (k.includes("walk") || k.includes("hik")) return "Walk / Hike";
  return "Strength / Other";
}

function isIndoorSport(typeKey: string): boolean {
  const k = typeKey.toLowerCase();
  return k.includes("treadmill") || k.includes("indoor") || k.includes("virtual");
}

function parseIsoDay(iso: string): Date {
  const d = iso.slice(0, 10);
  const [y, m, day] = d.split("-").map(Number);
  return new Date(y, (m || 1) - 1, day || 1);
}

function toLocalIso(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

function mondayStart(d: Date): Date {
  const copy = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const day = copy.getDay(); // 0=Sun..6=Sat
  const diff = day === 0 ? -6 : 1 - day;
  copy.setDate(copy.getDate() + diff);
  return copy;
}

function formatHm(sec: number | null | undefined): string {
  if (!sec || sec <= 0) return "–";
  const mins = Math.round(sec / 60);
  if (mins >= 60) return `${Math.floor(mins / 60)} h ${mins % 60} min`;
  return `${mins} min`;
}

function formatPace(secPerKm: number | null | undefined): string {
  if (!secPerKm || !Number.isFinite(secPerKm) || secPerKm <= 0) return "–";
  const total = Math.round(secPerKm);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}/km`;
}

export function TrendsView({ activities, days, loading, onOpenActivity }: TrendsViewProps) {
  const [range, setRange] = useState<TrendsRange>("1y");
  const [volumeMetric, setVolumeMetric] = useState<VolumeMetric>("km");
  const [calendarRunsOnly, setCalendarRunsOnly] = useState(true);
  const [hoveredWeek, setHoveredWeek] = useState<number | null>(null);

  const rangeDays = RANGE_OPTIONS.find((r) => r.id === range)?.days ?? null;

  const sinceIso = useMemo(() => {
    if (!rangeDays) return "0000-00-00";
    const d = new Date();
    d.setDate(d.getDate() - rangeDays);
    return toLocalIso(d);
  }, [rangeDays]);

  const filteredActivities = useMemo(
    () =>
      activities
        .filter((a) => a.start_iso.slice(0, 10) >= sinceIso)
        .sort((a, b) => a.start_iso.localeCompare(b.start_iso)),
    [activities, sinceIso]
  );

  const filteredDays = useMemo(
    () =>
      days
        .filter((d) => d.date >= sinceIso)
        .sort((a, b) => a.date.localeCompare(b.date)),
    [days, sinceIso]
  );

  const summary = useMemo(() => {
    const totalKm = filteredActivities.reduce((s, a) => s + a.distance_m / 1000, 0);
    const totalHours =
      filteredActivities.reduce(
        (s, a) => s + (a.moving_duration_sec > 0 ? a.moving_duration_sec : a.duration_sec),
        0
      ) / 3600;
    const totalClimb = filteredActivities
      .filter((a) => !isIndoorSport(a.type_key))
      .reduce((s, a) => s + Math.max(0, a.elevation_gain_m), 0);
    const sleepVals = filteredDays
      .map((d) => d.sleep_seconds ?? d.sleep_duration_sec)
      .filter((v): v is number => v != null && v > 0);
    const avgSleep = sleepVals.length
      ? sleepVals.reduce((a, b) => a + b, 0) / sleepVals.length
      : null;
    const hrvVals = filteredDays
      .map((d) => d.hrv_last_night)
      .filter((v): v is number => v != null && v > 0);
    const avgHrv = hrvVals.length
      ? Math.round(hrvVals.reduce((a, b) => a + b, 0) / hrvVals.length)
      : null;
    return {
      count: filteredActivities.length,
      totalKm,
      totalHours,
      totalClimb,
      avgSleep,
      avgHrv,
    };
  }, [filteredActivities, filteredDays]);

  // 53-week consistency heatmap + streaks
  const calendarModel = useMemo(() => {
    const shown = calendarRunsOnly
      ? activities.filter((a) => classifySport(a.type_key) === "Run")
      : activities;
    const today = new Date();
    const todayZero = new Date(today.getFullYear(), today.getMonth(), today.getDate());
    const startRef = new Date(todayZero);
    startRef.setDate(startRef.getDate() - 52 * 7);
    const firstWeek = mondayStart(startRef);

    const minutesByDay = new Map<string, number>();
    const activeDates = new Set<string>();
    const weekCounts = new Map<string, number>();

    for (const a of shown) {
      const iso = a.start_iso.slice(0, 10);
      if (!iso || iso.length < 10) continue;
      activeDates.add(iso);
      const mins = (a.moving_duration_sec > 0 ? a.moving_duration_sec : a.duration_sec) / 60;
      minutesByDay.set(iso, (minutesByDay.get(iso) ?? 0) + mins);
      const wk = toLocalIso(mondayStart(parseIsoDay(iso)));
      weekCounts.set(wk, (weekCounts.get(wk) ?? 0) + 1);
    }

    const weeks: { startIso: string; cells: { iso: string; mins: number; future: boolean }[] }[] =
      [];
    for (let w = 0; w < 53; w++) {
      const wkDate = new Date(firstWeek);
      wkDate.setDate(firstWeek.getDate() + w * 7);
      const cells = [];
      for (let d = 0; d < 7; d++) {
        const cellDate = new Date(wkDate);
        cellDate.setDate(wkDate.getDate() + d);
        const iso = toLocalIso(cellDate);
        cells.push({
          iso,
          mins: minutesByDay.get(iso) ?? 0,
          future: cellDate > todayZero,
        });
      }
      weeks.push({ startIso: toLocalIso(wkDate), cells });
    }

    // Current daily streak
    let currentDays = 0;
    const cursor = new Date(todayZero);
    if (!activeDates.has(toLocalIso(cursor))) {
      cursor.setDate(cursor.getDate() - 1);
    }
    while (activeDates.has(toLocalIso(cursor))) {
      currentDays += 1;
      cursor.setDate(cursor.getDate() - 1);
    }

    // Longest daily streak
    const sortedDays = Array.from(activeDates).sort();
    let longestDays = 0;
    let run = 0;
    let prevDate: Date | null = null;
    for (const iso of sortedDays) {
      const cur = parseIsoDay(iso);
      if (prevDate) {
        const diff = Math.round((cur.getTime() - prevDate.getTime()) / 86_400_000);
        run = diff === 1 ? run + 1 : 1;
      } else {
        run = 1;
      }
      if (run > longestDays) longestDays = run;
      prevDate = cur;
    }

    const thisWeekIso = toLocalIso(mondayStart(todayZero));
    const thisWeekCount = weekCounts.get(thisWeekIso) ?? 0;
    const bestWeekCount = Math.max(0, ...Array.from(weekCounts.values()));
    const activeDaysLastYear = weeks
      .flatMap((w) => w.cells)
      .filter((c) => !c.future && c.mins > 0).length;

    return {
      weeks,
      currentDays,
      longestDays,
      thisWeekCount,
      bestWeekCount,
      activeDaysLastYear,
    };
  }, [activities, calendarRunsOnly]);

  // Weekly volume, load & ACWR series
  const weeklySeries = useMemo(() => {
    const map = new Map<
      string,
      {
        weekIso: string;
        bySportKm: Record<SportBucket, number>;
        bySportHr: Record<SportBucket, number>;
        totalKm: number;
        totalHr: number;
        load: number;
      }
    >();

    const emptyBuckets = (): Record<SportBucket, number> => ({
      Run: 0,
      Ride: 0,
      Swim: 0,
      "Walk / Hike": 0,
      "Strength / Other": 0,
    });

    // Build across ALL activities first so ACWR has full 28d history, then slice to range
    const sortedAll = [...activities].sort((a, b) => a.start_iso.localeCompare(b.start_iso));
    if (sortedAll.length === 0) return [];

    const firstWk = mondayStart(parseIsoDay(sortedAll[0].start_iso));
    const lastWk = mondayStart(new Date());
    for (let d = new Date(firstWk); d <= lastWk; d.setDate(d.getDate() + 7)) {
      const iso = toLocalIso(d);
      map.set(iso, {
        weekIso: iso,
        bySportKm: emptyBuckets(),
        bySportHr: emptyBuckets(),
        totalKm: 0,
        totalHr: 0,
        load: 0,
      });
    }

    for (const a of sortedAll) {
      const wkIso = toLocalIso(mondayStart(parseIsoDay(a.start_iso)));
      const entry = map.get(wkIso);
      if (!entry) continue;
      const bucket = classifySport(a.type_key);
      const km = a.distance_m / 1000;
      const hr = (a.moving_duration_sec > 0 ? a.moving_duration_sec : a.duration_sec) / 3600;
      entry.bySportKm[bucket] += km;
      entry.bySportHr[bucket] += hr;
      entry.totalKm += km;
      entry.totalHr += hr;
      entry.load += a.training_load ?? 0;
    }

    const allWeeks = Array.from(map.values()).sort((a, b) => a.weekIso.localeCompare(b.weekIso));
    const withAcwr = allWeeks.map((w, idx) => {
      const startIdx = Math.max(0, idx - 4);
      const prev = allWeeks.slice(startIdx, idx);
      const chronicLoad =
        prev.length > 0 ? prev.reduce((s, p) => s + p.load, 0) / prev.length : 0;
      const chronicKm =
        prev.length > 0 ? prev.reduce((s, p) => s + p.totalKm, 0) / prev.length : 0;
      const acwr =
        chronicLoad > 1
          ? w.load / chronicLoad
          : chronicKm > 1
          ? w.totalKm / chronicKm
          : null;
      return { ...w, acwr };
    });

    const minWeekIso =
      rangeDays != null ? toLocalIso(mondayStart(parseIsoDay(sinceIso))) : "0000-00-00";
    return withAcwr.filter((w) => w.weekIso >= minWeekIso);
  }, [activities, rangeDays, sinceIso]);

  // Sport mix breakdown
  const sportMix = useMemo(() => {
    const map = new Map<SportBucket, number>();
    let totalSec = 0;
    for (const a of filteredActivities) {
      const bucket = classifySport(a.type_key);
      const sec = a.moving_duration_sec > 0 ? a.moving_duration_sec : a.duration_sec;
      map.set(bucket, (map.get(bucket) ?? 0) + sec);
      totalSec += sec;
    }
    const items = Array.from(map.entries())
      .map(([sport, sec]) => ({
        sport,
        sec,
        pct: totalSec > 0 ? (sec / totalSec) * 100 : 0,
        color: SPORT_COLORS[sport],
      }))
      .sort((a, b) => b.sec - a.sec);
    return { items, totalHours: totalSec / 3600 };
  }, [filteredActivities]);

  // Aerobic form (pace scaled to reference HR with 6x elevation factor + 30-day median trend)
  const aerobicForm = useMemo(() => {
    const rhrs = days
      .map((d) => d.resting_hr)
      .filter((v): v is number => v != null && v > 0)
      .sort((a, b) => a - b);
    const rest = rhrs.length > 0 ? rhrs[Math.floor(rhrs.length / 2)] : 55;

    const usable = activities
      .filter((a) => {
        const dur = a.moving_duration_sec > 0 ? a.moving_duration_sec : a.duration_sec;
        return (
          classifySport(a.type_key) === "Run" &&
          !isIndoorSport(a.type_key) &&
          a.distance_m >= 3000 &&
          dur >= 900 &&
          (a.average_hr ?? 0) >= rest + 30
        );
      })
      .sort((a, b) => a.start_iso.localeCompare(b.start_iso));

    if (usable.length === 0) return { reference: 150, resting: rest, points: [], shown: [] };

    const hrs = usable.map((a) => a.average_hr!).sort((a, b) => a - b);
    const medHr = hrs[Math.floor(hrs.length / 2)];
    const reference = Math.round(medHr / 5) * 5;

    const points: { id: number; dateIso: string; ms: number; pace: number; trend: number }[] = [];
    for (const a of usable) {
      const hr = a.average_hr!;
      const dur = a.moving_duration_sec > 0 ? a.moving_duration_sec : a.duration_sec;
      if (dur <= 0 || hr <= rest) continue;
      const effectiveM = a.distance_m + 6 * Math.max(0, a.elevation_gain_m);
      const speed = (effectiveM / dur) * ((reference - rest) / (hr - rest));
      if (speed <= 1.5 || speed >= 7.0) continue;
      points.push({
        id: a.garmin_id,
        dateIso: a.start_iso.slice(0, 10),
        ms: parseIsoDay(a.start_iso).getTime(),
        pace: 1000 / speed,
        trend: 0,
      });
    }

    for (let i = 0; i < points.length; i++) {
      const cutoff = points[i].ms - 30 * 86_400_000;
      const win = points
        .slice(0, i + 1)
        .filter((p) => p.ms >= cutoff)
        .map((p) => p.pace)
        .sort((a, b) => a - b);
      points[i].trend = win[Math.floor(win.length / 2)];
    }

    const shown = points.filter((p) => p.dateIso >= sinceIso);
    return { reference, resting: rest, points, shown };
  }, [activities, days, sinceIso]);

  // Resting HR & HRV multi-week curves
  const recoverySeries = useMemo(() => {
    const sorted = [...days].sort((a, b) => a.date.localeCompare(b.date));
    const rhrPts: { date: string; val: number; avg7: number }[] = [];
    const hrvPts: { date: string; val: number; avg7: number }[] = [];
    for (let i = 0; i < sorted.length; i++) {
      const d = sorted[i];
      if (d.resting_hr && d.resting_hr > 0) {
        const win = sorted
          .slice(Math.max(0, i - 6), i + 1)
          .map((x) => x.resting_hr)
          .filter((v): v is number => v != null && v > 0);
        rhrPts.push({
          date: d.date,
          val: d.resting_hr,
          avg7: win.reduce((a, b) => a + b, 0) / win.length,
        });
      }
      if (d.hrv_last_night && d.hrv_last_night > 0) {
        const win = sorted
          .slice(Math.max(0, i - 6), i + 1)
          .map((x) => x.hrv_last_night)
          .filter((v): v is number => v != null && v > 0);
        hrvPts.push({
          date: d.date,
          val: d.hrv_last_night,
          avg7: win.reduce((a, b) => a + b, 0) / win.length,
        });
      }
    }
    return {
      rhr: rhrPts.filter((p) => p.date >= sinceIso),
      hrv: hrvPts.filter((p) => p.date >= sinceIso),
    };
  }, [days, sinceIso]);

  if (loading && activities.length === 0) {
    return (
      <div className="card">
        <h3 style={{ marginTop: 0 }}>Trends</h3>
        <div className="muted">Loading training history…</div>
      </div>
    );
  }

  if (activities.length === 0 && days.length === 0) {
    return (
      <div className="card">
        <h3 style={{ marginTop: 0 }}>Trends</h3>
        <div className="muted">
          No training history synced yet. Use <strong>Sync Watch</strong> in the sidebar to load your workouts and wellness trends.
        </div>
      </div>
    );
  }

  const maxVol = Math.max(
    1,
    ...weeklySeries.map((w) => (volumeMetric === "km" ? w.totalKm : w.totalHr))
  );
  const maxLoad = Math.max(1, ...weeklySeries.map((w) => w.load));
  const acwrPts = weeklySeries
    .map((w, idx) => ({ idx, weekIso: w.weekIso, acwr: w.acwr }))
    .filter((p): p is { idx: number; weekIso: string; acwr: number } => p.acwr != null);

  return (
    <div>
      <div className="page-header">
        <div className="page-header-left">
          <h2 className="today-title">Trends</h2>
          <span className="today-subtitle">
            {range === "all"
              ? "Everything since your first synced activity"
              : `Last ${RANGE_OPTIONS.find((r) => r.id === range)?.label}`}
          </span>
        </div>
        <div className="segmented-control" role="tablist" aria-label="Trends timeframe">
          {RANGE_OPTIONS.map((opt) => (
            <button
              key={opt.id}
              type="button"
              className={`seg-btn ${range === opt.id ? "active" : ""}`}
              onClick={() => setRange(opt.id)}
            >
              {opt.label}
            </button>
          ))}
        </div>
      </div>

      {/* Summary KPIs */}
      <div className="kpi-grid">
        <div className="kpi-tile">
          <span className="kpi-tile-label">Activities</span>
          <span className="kpi-tile-val">{summary.count.toLocaleString()}</span>
          <span className="kpi-tile-note">sessions in window</span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Distance</span>
          <span className="kpi-tile-val">{summary.totalKm.toFixed(0)} km</span>
          <span className="kpi-tile-note">across all sports</span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Time Moving</span>
          <span className="kpi-tile-val">{Math.round(summary.totalHours)} h</span>
          <span className="kpi-tile-note">{(summary.totalHours * 60).toFixed(0)} min</span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Climbed</span>
          <span className="kpi-tile-val">{Math.round(summary.totalClimb).toLocaleString()} m</span>
          <span className="kpi-tile-note">outdoor elevation gain</span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Avg Sleep</span>
          <span className="kpi-tile-val">{formatHm(summary.avgSleep)}</span>
          <span className="kpi-tile-note">per night</span>
        </div>
        <div className="kpi-tile">
          <span className="kpi-tile-label">Avg HRV</span>
          <span className="kpi-tile-val">{summary.avgHrv != null ? `${summary.avgHrv} ms` : "–"}</span>
          <span className="kpi-tile-note">overnight average</span>
        </div>
      </div>

      {/* 53-Week Activity Consistency Heatmap */}
      <div className="card">
        <div className="card-header">
          <div>
            <strong>Last 12 Months</strong>
            <div className="muted" style={{ fontSize: 12 }}>
              {calendarModel.activeDaysLastYear} active days · Current streak {calendarModel.currentDays}d (best{" "}
              {calendarModel.longestDays}d) · This week {calendarModel.thisWeekCount} (best{" "}
              {calendarModel.bestWeekCount})
            </div>
          </div>
          <div className="segmented-control">
            <button
              type="button"
              className={`seg-btn ${calendarRunsOnly ? "active" : ""}`}
              onClick={() => setCalendarRunsOnly(true)}
            >
              Running
            </button>
            <button
              type="button"
              className={`seg-btn ${!calendarRunsOnly ? "active" : ""}`}
              onClick={() => setCalendarRunsOnly(false)}
            >
              Everything
            </button>
          </div>
        </div>

        <div style={{ overflowX: "auto", paddingBottom: 4 }}>
          <svg viewBox="0 0 760 118" style={{ width: "100%", minWidth: 620, height: "auto", display: "block" }}>
            {["Mon", "Wed", "Fri", "Sun"].map((lbl, idx) => (
              <text
                key={lbl}
                x={0}
                y={22 + idx * 2 * 13}
                fontSize={9}
                fill="var(--muted)"
              >
                {lbl}
              </text>
            ))}
            {calendarModel.weeks.map((wk, wIdx) => {
              const x = 32 + wIdx * 13.7;
              const monthLabel =
                wIdx === 0 ||
                wk.startIso.slice(5, 7) !== calendarModel.weeks[wIdx - 1]?.startIso.slice(5, 7)
                  ? parseIsoDay(wk.startIso).toLocaleDateString(undefined, { month: "short" })
                  : null;
              return (
                <g key={wk.startIso}>
                  {monthLabel && wIdx < 51 && (
                    <text x={x} y={9} fontSize={9} fill="var(--muted)">
                      {monthLabel}
                    </text>
                  )}
                  {wk.cells.map((cell, dIdx) => {
                    if (cell.future) return null;
                    const opacity =
                      cell.mins <= 0
                        ? 0.08
                        : cell.mins < 30
                        ? 0.3
                        : cell.mins < 60
                        ? 0.55
                        : cell.mins < 100
                        ? 0.78
                        : 1;
                    return (
                      <rect
                        key={cell.iso}
                        x={x}
                        y={14 + dIdx * 13.5}
                        width={11}
                        height={11}
                        rx={2.5}
                        fill={cell.mins <= 0 ? "var(--text)" : "var(--accent-primary)"}
                        fillOpacity={opacity}
                      >
                        <title>{`${cell.iso}: ${Math.round(cell.mins)} min`}</title>
                      </rect>
                    );
                  })}
                </g>
              );
            })}
          </svg>
        </div>
      </div>

      {/* Weekly Volume + Sport Mix */}
      <div className="two-col-grid">
        <div className="card" style={{ marginBottom: 0 }}>
          <div className="card-header">
            <div>
              <strong>Weekly Volume</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                Stacked by sport ({volumeMetric === "km" ? "km" : "hours"} / week)
              </div>
            </div>
            <div className="segmented-control">
              <button
                type="button"
                className={`seg-btn ${volumeMetric === "km" ? "active" : ""}`}
                onClick={() => setVolumeMetric("km")}
              >
                km
              </button>
              <button
                type="button"
                className={`seg-btn ${volumeMetric === "hours" ? "active" : ""}`}
                onClick={() => setVolumeMetric("hours")}
              >
                Hours
              </button>
            </div>
          </div>
          {weeklySeries.length === 0 ? (
            <div className="muted">No weekly volume in this range.</div>
          ) : (
            <div className="svg-chart-wrap" onMouseLeave={() => setHoveredWeek(null)}>
              <svg viewBox="0 0 640 210" style={{ width: "100%", height: "auto", display: "block" }}>
                {[0.25, 0.5, 0.75, 1].map((frac) => {
                  const y = 175 - frac * 155;
                  return (
                    <g key={frac}>
                      <line x1={38} y1={y} x2={630} y2={y} stroke="var(--border)" strokeDasharray="3 3" />
                      <text x={32} y={y + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                        {(maxVol * frac).toFixed(volumeMetric === "km" ? 0 : 1)}
                      </text>
                    </g>
                  );
                })}
                {weeklySeries.map((wk, idx) => {
                  const slotW = (630 - 42) / Math.max(weeklySeries.length, 1);
                  const barW = Math.max(2, Math.min(22, slotW * 0.72));
                  const x = 42 + idx * slotW + (slotW - barW) / 2;
                  let curY = 175;
                  const buckets: SportBucket[] = ["Run", "Ride", "Swim", "Walk / Hike", "Strength / Other"];
                  return (
                    <g key={wk.weekIso} onMouseEnter={() => setHoveredWeek(idx)}>
                      <rect x={42 + idx * slotW} y={14} width={slotW} height={166} fill="transparent" />
                      {buckets.map((b) => {
                        const val = volumeMetric === "km" ? wk.bySportKm[b] : wk.bySportHr[b];
                        if (val <= 0) return null;
                        const h = (val / maxVol) * 155;
                        curY -= h;
                        return (
                          <rect
                            key={b}
                            x={x}
                            y={curY}
                            width={barW}
                            height={Math.max(1, h)}
                            rx={1.5}
                            fill={SPORT_COLORS[b]}
                            opacity={hoveredWeek === null || hoveredWeek === idx ? 0.92 : 0.45}
                          />
                        );
                      })}
                    </g>
                  );
                })}
                {weeklySeries.length > 0 && (
                  <>
                    <text x={42} y={195} fontSize={9.5} fill="var(--muted)">
                      {weeklySeries[0].weekIso}
                    </text>
                    <text x={630} y={195} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                      {weeklySeries[weeklySeries.length - 1].weekIso}
                    </text>
                  </>
                )}
              </svg>
              {hoveredWeek !== null && weeklySeries[hoveredWeek] && (
                <div className="chart-tooltip" style={{ top: 8, right: 8 }}>
                  <strong>Week of {weeklySeries[hoveredWeek].weekIso}</strong>
                  <div>
                    {weeklySeries[hoveredWeek].totalKm.toFixed(1)} km ·{" "}
                    {weeklySeries[hoveredWeek].totalHr.toFixed(1)} h · Load{" "}
                    {Math.round(weeklySeries[hoveredWeek].load)}
                  </div>
                </div>
              )}
              <div className="chart-legend">
                {(Object.keys(SPORT_COLORS) as SportBucket[]).map((b) => (
                  <span key={b} className="legend-item">
                    <span className="legend-swatch" style={{ background: SPORT_COLORS[b] }} />
                    {b}
                  </span>
                ))}
              </div>
            </div>
          )}
        </div>

        <div className="card" style={{ marginBottom: 0 }}>
          <div className="card-header">
            <div>
              <strong>Where Your Time Went</strong>
              <div className="muted" style={{ fontSize: 12 }}>
                {sportMix.totalHours.toFixed(1)} hours across {sportMix.items.length} sport groups
              </div>
            </div>
          </div>
          {sportMix.items.length === 0 ? (
            <div className="muted">No activities in this timeframe.</div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              {sportMix.items.map((item) => (
                <div key={item.sport}>
                  <div style={{ display: "flex", justifyContent: "space-between", fontSize: 13, marginBottom: 4 }}>
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 7 }}>
                      <span className="legend-swatch" style={{ background: item.color }} />
                      <strong>{item.sport}</strong>
                    </span>
                    <span className="muted">
                      {(item.sec / 3600).toFixed(1)} h ({item.pct.toFixed(0)}%)
                    </span>
                  </div>
                  <div className="envelope-bar-track">
                    <div
                      className="envelope-bar-fill"
                      style={{ width: `${Math.max(2, item.pct)}%`, background: item.color }}
                    />
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      {/* Training Load & ACWR */}
      <div className="card">
        <div className="card-header">
          <div>
            <strong>Weekly Training Load & ACWR</strong>
            <div className="muted" style={{ fontSize: 12 }}>
              Garmin training load bars with rolling Acute:Chronic Workload Ratio (sweet spot 0.8–1.3×)
            </div>
          </div>
          {acwrPts.length > 0 && (
            <span
              className={`pill ${
                acwrPts[acwrPts.length - 1].acwr > 1.5
                  ? "red"
                  : acwrPts[acwrPts.length - 1].acwr > 1.3
                  ? "amber"
                  : "green"
              }`}
            >
              Latest ACWR {acwrPts[acwrPts.length - 1].acwr.toFixed(2)}×
            </span>
          )}
        </div>
        {weeklySeries.length === 0 ? (
          <div className="muted">No training load data in this range.</div>
        ) : (
          <div className="svg-chart-wrap">
            <svg viewBox="0 0 880 210" style={{ width: "100%", height: "auto", display: "block" }}>
              {/* ACWR 0.8 - 1.3 sweet spot band (right axis 0..2.2) */}
              {(() => {
                const yTop = 175 - (1.3 / 2.2) * 150;
                const yBot = 175 - (0.8 / 2.2) * 150;
                const yWarn = 175 - (1.5 / 2.2) * 150;
                return (
                  <>
                    <rect
                      x={46}
                      y={yTop}
                      width={788}
                      height={yBot - yTop}
                      fill="var(--accent-primary)"
                      opacity={0.1}
                    />
                    <line
                      x1={46}
                      y1={yWarn}
                      x2={834}
                      y2={yWarn}
                      stroke="var(--amber)"
                      strokeDasharray="4 4"
                      opacity={0.75}
                    />
                    <text x={840} y={yWarn + 3} fontSize={9.5} fill="var(--amber)">
                      1.5×
                    </text>
                    <text x={840} y={yTop + 3} fontSize={9.5} fill="var(--accent-primary)">
                      1.3×
                    </text>
                    <text x={840} y={yBot + 3} fontSize={9.5} fill="var(--accent-primary)">
                      0.8×
                    </text>
                  </>
                );
              })()}
              {[0.5, 1].map((frac) => {
                const y = 175 - frac * 150;
                return (
                  <g key={frac}>
                    <line x1={46} y1={y} x2={834} y2={y} stroke="var(--border)" strokeDasharray="3 3" />
                    <text x={40} y={y + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                      {Math.round(maxLoad * frac)}
                    </text>
                  </g>
                );
              })}
              {weeklySeries.map((wk, idx) => {
                const slotW = (834 - 46) / Math.max(weeklySeries.length, 1);
                const barW = Math.max(2, Math.min(20, slotW * 0.65));
                const x = 46 + idx * slotW + (slotW - barW) / 2;
                const h = (wk.load / maxLoad) * 150;
                return (
                  <rect
                    key={wk.weekIso}
                    x={x}
                    y={175 - h}
                    width={barW}
                    height={Math.max(1, h)}
                    rx={2}
                    fill="var(--chart-teal)"
                    opacity={0.42}
                  >
                    <title>{`${wk.weekIso}: Load ${Math.round(wk.load)}${
                      wk.acwr != null ? ` · ACWR ${wk.acwr.toFixed(2)}×` : ""
                    }`}</title>
                  </rect>
                );
              })}
              {acwrPts.length > 1 && (
                <polyline
                  fill="none"
                  stroke="var(--accent-primary)"
                  strokeWidth={2.4}
                  points={acwrPts
                    .map((p) => {
                      const slotW = (834 - 46) / Math.max(weeklySeries.length, 1);
                      const x = 46 + p.idx * slotW + slotW / 2;
                      const clamped = Math.min(2.2, Math.max(0, p.acwr));
                      const y = 175 - (clamped / 2.2) * 150;
                      return `${x.toFixed(1)},${y.toFixed(1)}`;
                    })
                    .join(" ")}
                />
              )}
              {weeklySeries.length > 0 && (
                <>
                  <text x={46} y={195} fontSize={9.5} fill="var(--muted)">
                    {weeklySeries[0].weekIso}
                  </text>
                  <text x={834} y={195} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                    {weeklySeries[weeklySeries.length - 1].weekIso}
                  </text>
                </>
              )}
            </svg>
            <div className="chart-legend">
              <span className="legend-item">
                <span className="legend-swatch" style={{ background: "var(--chart-teal)", opacity: 0.6 }} />
                Weekly load
              </span>
              <span className="legend-item">
                <span className="legend-line" style={{ background: "var(--accent-primary)" }} />
                ACWR (7d / 28d avg)
              </span>
              <span className="legend-item">
                <span className="legend-swatch" style={{ background: "var(--accent-primary)", opacity: 0.2 }} />
                0.8–1.3× optimal zone
              </span>
            </div>
          </div>
        )}
      </div>

      {/* Aerobic Efficiency / Pace Form */}
      <div className="card">
        <div className="card-header">
          <div>
            <strong>Aerobic Efficiency & Pace Form</strong>
            <div className="muted" style={{ fontSize: 12 }}>
              Running pace scaled to {aerobicForm.reference} bpm (RHR {aerobicForm.resting} bpm, 6× climb adjustment) · Click any dot to open run
            </div>
          </div>
          {aerobicForm.points.length > 0 && (
            <strong style={{ fontSize: 18 }}>
              {formatPace(aerobicForm.points[aerobicForm.points.length - 1].trend)} @ {aerobicForm.reference} bpm
            </strong>
          )}
        </div>
        {aerobicForm.shown.length < 2 ? (
          <div className="muted">
            Needs at least 2 outdoor runs of 3 km+ and 15 min+ with heart rate in this timeframe.
          </div>
        ) : (
          (() => {
            const paces = aerobicForm.shown.map((p) => p.pace);
            const minP = Math.min(...paces) - 10; // fastest (top)
            const maxP = Math.max(...paces) + 10; // slowest (bottom)
            const spanP = Math.max(20, maxP - minP);
            const minMs = aerobicForm.shown[0].ms;
            const maxMs = aerobicForm.shown[aerobicForm.shown.length - 1].ms;
            const spanMs = Math.max(86_400_000, maxMs - minMs);
            const toX = (ms: number) => 56 + ((ms - minMs) / spanMs) * 790;
            // Higher on chart = faster (smaller sec/km)
            const toY = (pace: number) => 22 + ((pace - minP) / spanP) * 155;

            return (
              <div className="svg-chart-wrap">
                <svg viewBox="0 0 880 205" style={{ width: "100%", height: "auto", display: "block" }}>
                  {[0, 0.5, 1].map((f) => {
                    const p = minP + f * spanP;
                    const y = toY(p);
                    return (
                      <g key={f}>
                        <line x1={56} y1={y} x2={846} y2={y} stroke="var(--border)" strokeDasharray="3 3" />
                        <text x={50} y={y + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                          {formatPace(p).replace("/km", "")}
                        </text>
                      </g>
                    );
                  })}
                  {aerobicForm.shown.map((p) => (
                    <circle
                      key={p.id}
                      cx={toX(p.ms)}
                      cy={toY(p.pace)}
                      r={4}
                      fill="var(--accent-primary)"
                      fillOpacity={0.38}
                      style={{ cursor: "pointer" }}
                      onClick={() => onOpenActivity(p.id)}
                    >
                      <title>{`${p.dateIso}: ${formatPace(p.pace)} @ ${aerobicForm.reference} bpm`}</title>
                    </circle>
                  ))}
                  <polyline
                    fill="none"
                    stroke="var(--accent-primary)"
                    strokeWidth={2.5}
                    points={aerobicForm.shown
                      .map((p) => `${toX(p.ms).toFixed(1)},${toY(p.trend).toFixed(1)}`)
                      .join(" ")}
                  />
                  <text x={56} y={196} fontSize={9.5} fill="var(--muted)">
                    {aerobicForm.shown[0].dateIso}
                  </text>
                  <text x={846} y={196} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                    {aerobicForm.shown[aerobicForm.shown.length - 1].dateIso}
                  </text>
                </svg>
                <div className="muted" style={{ fontSize: 12, marginTop: 4 }}>
                  Higher on the chart means faster running at the same heart rate (30-day rolling median trend).
                </div>
              </div>
            );
          })()
        )}
      </div>

      {/* Resting HR & Overnight HRV Multi-Week Curves */}
      <div className="two-col-grid">
        <div className="card" style={{ marginBottom: 0 }}>
          <div className="card-header">
            <div>
              <strong>Resting Heart Rate Trend</strong>
              <div className="muted" style={{ fontSize: 12 }}>Daily readings & 7-day rolling mean</div>
            </div>
            {recoverySeries.rhr.length > 0 && (
              <span className="pill">
                Latest {recoverySeries.rhr[recoverySeries.rhr.length - 1].val} bpm
              </span>
            )}
          </div>
          {recoverySeries.rhr.length < 2 ? (
            <div className="muted">Not enough resting HR readings in this range.</div>
          ) : (
            (() => {
              const vals = recoverySeries.rhr.map((p) => p.val);
              const lo = Math.min(...vals) - 2;
              const hi = Math.max(...vals) + 2;
              const span = Math.max(4, hi - lo);
              const toX = (i: number) =>
                40 + (i / Math.max(1, recoverySeries.rhr.length - 1)) * 580;
              const toY = (v: number) => 165 - ((v - lo) / span) * 145;
              return (
                <svg viewBox="0 0 640 190" style={{ width: "100%", height: "auto", display: "block" }}>
                  {[0, 0.5, 1].map((f) => {
                    const v = lo + f * span;
                    const y = toY(v);
                    return (
                      <g key={f}>
                        <line x1={40} y1={y} x2={620} y2={y} stroke="var(--border)" strokeDasharray="3 3" />
                        <text x={34} y={y + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                          {Math.round(v)}
                        </text>
                      </g>
                    );
                  })}
                  {recoverySeries.rhr.map((p, i) => (
                    <circle key={p.date} cx={toX(i)} cy={toY(p.val)} r={2.3} fill="var(--chart-coral)" opacity={0.45}>
                      <title>{`${p.date}: ${p.val} bpm`}</title>
                    </circle>
                  ))}
                  <polyline
                    fill="none"
                    stroke="var(--chart-coral)"
                    strokeWidth={2.3}
                    points={recoverySeries.rhr.map((p, i) => `${toX(i).toFixed(1)},${toY(p.avg7).toFixed(1)}`).join(" ")}
                  />
                  <text x={40} y={182} fontSize={9.5} fill="var(--muted)">
                    {recoverySeries.rhr[0].date}
                  </text>
                  <text x={620} y={182} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                    {recoverySeries.rhr[recoverySeries.rhr.length - 1].date}
                  </text>
                </svg>
              );
            })()
          )}
        </div>

        <div className="card" style={{ marginBottom: 0 }}>
          <div className="card-header">
            <div>
              <strong>Overnight HRV Trend</strong>
              <div className="muted" style={{ fontSize: 12 }}>Daily ms & 7-day rolling mean</div>
            </div>
            {recoverySeries.hrv.length > 0 && (
              <span className="pill green">
                Latest {recoverySeries.hrv[recoverySeries.hrv.length - 1].val} ms
              </span>
            )}
          </div>
          {recoverySeries.hrv.length < 2 ? (
            <div className="muted">Not enough overnight HRV readings in this range.</div>
          ) : (
            (() => {
              const vals = recoverySeries.hrv.map((p) => p.val);
              const lo = Math.max(0, Math.min(...vals) - 4);
              const hi = Math.max(...vals) + 4;
              const span = Math.max(8, hi - lo);
              const toX = (i: number) =>
                40 + (i / Math.max(1, recoverySeries.hrv.length - 1)) * 580;
              const toY = (v: number) => 165 - ((v - lo) / span) * 145;
              return (
                <svg viewBox="0 0 640 190" style={{ width: "100%", height: "auto", display: "block" }}>
                  {[0, 0.5, 1].map((f) => {
                    const v = lo + f * span;
                    const y = toY(v);
                    return (
                      <g key={f}>
                        <line x1={40} y1={y} x2={620} y2={y} stroke="var(--border)" strokeDasharray="3 3" />
                        <text x={34} y={y + 3} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                          {Math.round(v)}
                        </text>
                      </g>
                    );
                  })}
                  {recoverySeries.hrv.map((p, i) => (
                    <circle key={p.date} cx={toX(i)} cy={toY(p.val)} r={2.3} fill="var(--chart-teal)" opacity={0.45}>
                      <title>{`${p.date}: ${p.val} ms`}</title>
                    </circle>
                  ))}
                  <polyline
                    fill="none"
                    stroke="var(--chart-teal)"
                    strokeWidth={2.3}
                    points={recoverySeries.hrv.map((p, i) => `${toX(i).toFixed(1)},${toY(p.avg7).toFixed(1)}`).join(" ")}
                  />
                  <text x={40} y={182} fontSize={9.5} fill="var(--muted)">
                    {recoverySeries.hrv[0].date}
                  </text>
                  <text x={620} y={182} textAnchor="end" fontSize={9.5} fill="var(--muted)">
                    {recoverySeries.hrv[recoverySeries.hrv.length - 1].date}
                  </text>
                </svg>
              );
            })()
          )}
        </div>
      </div>
    </div>
  );
}
