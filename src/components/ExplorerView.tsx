import { useMemo, useRef, useState } from "react";
import { Compass, Download, Minus, Plus, RefreshCw, RotateCcw } from "lucide-react";
import worldCountriesRaw from "../data/worldCountries.json";
import type {
  ExplorerCityVisit,
  ExplorerCountryVisit,
  ExplorerSportFilter,
  ExplorerSummaryPayload,
  ExplorerTally,
} from "../types";
import { CountryFlag } from "./CountryFlag";

interface WorldCountryFeature {
  id: string;
  iso: string;
  name: string;
  continent: string;
  label: [number, number];
  rings: number[][];
}

const WORLD_COUNTRIES = worldCountriesRaw as WorldCountryFeature[];

interface ExplorerViewProps {
  summary: ExplorerSummaryPayload | null;
  sportFilter: ExplorerSportFilter;
  onChangeSportFilter: (next: ExplorerSportFilter) => void;
  loading: boolean;
  error: string | null;
  onRefresh: () => void;
  hasGarminKey: boolean;
  lifetimeBackfillCompleted: boolean;
  backfillBusy: boolean;
  onBackfillHistory: () => void;
}

const MAP_W = 1000;
const MAP_H = 540;
const MIN_LAT = -58;
const MAX_LAT = 82;

function mercatorY(latDeg: number): number {
  const clamped = Math.max(MIN_LAT, Math.min(MAX_LAT, latDeg));
  const rad = (clamped * Math.PI) / 180;
  return Math.log(Math.tan(Math.PI / 4 + rad / 2));
}

const MERC_Y_MIN = mercatorY(MIN_LAT);
const MERC_Y_MAX = mercatorY(MAX_LAT);

function projectLonLat(lon: number, lat: number): [number, number] {
  const x = ((lon + 180) / 360) * MAP_W;
  const my = mercatorY(lat);
  const y = ((MERC_Y_MAX - my) / (MERC_Y_MAX - MERC_Y_MIN)) * MAP_H;
  return [x, y];
}

function buildRingPath(flatRing: number[]): string {
  if (flatRing.length < 6) return "";
  const parts: string[] = [];
  let prevLon = flatRing[0];
  const [x0, y0] = projectLonLat(flatRing[0], flatRing[1]);
  parts.push(`M${x0.toFixed(1)},${y0.toFixed(1)}`);
  let segmentOpen = true;
  for (let i = 2; i < flatRing.length - 1; i += 2) {
    const lon = flatRing[i];
    const lat = flatRing[i + 1];
    const [x, y] = projectLonLat(lon, lat);
    if (Math.abs(lon - prevLon) > 180) {
      parts.push(`M${x.toFixed(1)},${y.toFixed(1)}`);
      segmentOpen = false;
    } else {
      parts.push(`L${x.toFixed(1)},${y.toFixed(1)}`);
      segmentOpen = true;
    }
    prevLon = lon;
  }
  if (segmentOpen) {
    parts.push("Z");
  }
  return parts.join("");
}

function buildGeodesicPath(lat1: number, lon1: number, lat2: number, lon2: number): string {
  const toRad = Math.PI / 180;
  const toDeg = 180 / Math.PI;
  const p1 = lat1 * toRad;
  const l1 = lon1 * toRad;
  const p2 = lat2 * toRad;
  const l2 = lon2 * toRad;

  const v1: [number, number, number] = [
    Math.cos(p1) * Math.cos(l1),
    Math.cos(p1) * Math.sin(l1),
    Math.sin(p1),
  ];
  const v2: [number, number, number] = [
    Math.cos(p2) * Math.cos(l2),
    Math.cos(p2) * Math.sin(l2),
    Math.sin(p2),
  ];
  const dot = Math.max(-1, Math.min(1, v1[0] * v2[0] + v1[1] * v2[1] + v1[2] * v2[2]));
  const omega = Math.acos(dot);
  const steps = 24;
  const parts: string[] = [];
  let prevLon: number | null = null;

  for (let i = 0; i <= steps; i++) {
    const t = i / steps;
    let x: number;
    let y: number;
    let z: number;
    if (omega < 1e-4) {
      x = v1[0] + (v2[0] - v1[0]) * t;
      y = v1[1] + (v2[1] - v1[1]) * t;
      z = v1[2] + (v2[2] - v1[2]) * t;
    } else {
      const sinO = Math.sin(omega);
      const a = Math.sin((1 - t) * omega) / sinO;
      const b = Math.sin(t * omega) / sinO;
      x = a * v1[0] + b * v2[0];
      y = a * v1[1] + b * v2[1];
      z = a * v1[2] + b * v2[2];
    }
    const norm = Math.hypot(x, y, z) || 1;
    const lat = Math.asin(Math.max(-1, Math.min(1, z / norm))) * toDeg;
    const lon = Math.atan2(y, x) * toDeg;
    const [px, py] = projectLonLat(lon, lat);
    if (prevLon === null || Math.abs(lon - prevLon) > 180) {
      parts.push(`M${px.toFixed(1)},${py.toFixed(1)}`);
    } else {
      parts.push(`L${px.toFixed(1)},${py.toFixed(1)}`);
    }
    prevLon = lon;
  }
  return parts.join("");
}

const PREBUILT_COUNTRY_PATHS = WORLD_COUNTRIES.map((c) => ({
  id: c.id,
  iso: c.iso,
  name: c.name,
  continent: c.continent,
  d: c.rings.map(buildRingPath).join(" "),
}));

function formatMonthYear(iso: string): string {
  const m = iso.match(/^(\d{4})-(\d{2})/);
  if (!m) return iso.slice(0, 10);
  const dt = new Date(Number(m[1]), Number(m[2]) - 1, 1);
  return dt.toLocaleDateString(undefined, { month: "long", year: "numeric" });
}

function formatDayMonthYear(iso: string): string {
  const m = iso.match(/^(\d{4})-(\d{2})-(\d{2})/);
  if (!m) return iso.slice(0, 10);
  const dt = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  return dt.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

function nounForFilter(filter: ExplorerSportFilter, n: number): string {
  if (filter === "run") return n === 1 ? "run" : "runs";
  if (filter === "ride") return n === 1 ? "ride" : "rides";
  return n === 1 ? "activity" : "activities";
}

function accentColorForFilter(filter: ExplorerSportFilter): string {
  if (filter === "ride") return "#F5B731";
  if (filter === "all") return "#60A5FA";
  return "var(--accent-primary)";
}

export function ExplorerView({
  summary,
  sportFilter,
  onChangeSportFilter,
  loading,
  error,
  onRefresh,
  hasGarminKey,
  lifetimeBackfillCompleted,
  backfillBusy,
  onBackfillHistory,
}: ExplorerViewProps) {
  const [selectedYear, setSelectedYear] = useState<number | null>(null);
  const [allCitiesShown, setAllCitiesShown] = useState(false);

  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  const dragStartRef = useRef<{ clientX: number; clientY: number; panX: number; panY: number } | null>(null);
  const mapContainerRef = useRef<HTMLDivElement | null>(null);
  const [tooltip, setTooltip] = useState<{ text: string; x: number; y: number } | null>(null);

  const activeYearObj = useMemo(() => {
    if (selectedYear == null || !summary) return null;
    return summary.years.find((y) => y.year === selectedYear) ?? null;
  }, [selectedYear, summary]);

  const effectiveYear = activeYearObj ? activeYearObj.year : null;
  const sportColor = accentColorForFilter(sportFilter);

  const visitsById = useMemo(() => {
    const map = new Map<string, ExplorerCountryVisit>();
    for (const v of summary?.visits ?? []) {
      map.set(v.id, v);
    }
    return map;
  }, [summary]);

  const shownCountryIds = useMemo(() => {
    if (!summary) return new Set<string>();
    if (activeYearObj) return new Set(activeYearObj.country_ids);
    return new Set(summary.visits.map((v) => v.id));
  }, [summary, activeYearObj]);

  const shownCountries = useMemo(() => {
    if (!summary) return [];
    if (activeYearObj) {
      return activeYearObj.country_ids
        .map((id) => visitsById.get(id))
        .filter((v): v is ExplorerCountryVisit => Boolean(v));
    }
    return summary.visits;
  }, [summary, activeYearObj, visitsById]);

  const shownCities = useMemo(() => {
    if (!summary) return [];
    if (!activeYearObj) return summary.cities;
    const set = new Set(activeYearObj.city_ids);
    return summary.cities.filter((c) => set.has(c.city_id));
  }, [summary, activeYearObj]);

  const shownLegs = useMemo(() => {
    if (!summary) return [];
    const base = activeYearObj
      ? summary.legs.filter(
          (l) =>
            l.year === activeYearObj.year &&
            shownCountryIds.has(l.from_country_id) &&
            shownCountryIds.has(l.to_country_id)
        )
      : summary.legs;
    const seen = new Set<string>();
    const unique: typeof base = [];
    for (const leg of base) {
      const key = [leg.from_country_id, leg.to_country_id].sort().join("|");
      if (!seen.has(key)) {
        seen.add(key);
        unique.push(leg);
      }
    }
    return unique;
  }, [summary, activeYearObj, shownCountryIds]);

  const geodesicPaths = useMemo(
    () =>
      shownLegs.map((l) => ({
        key: `${l.from_country_id}-${l.to_country_id}-${l.date_iso}`,
        d: buildGeodesicPath(l.from_lat, l.from_lon, l.to_lat, l.to_lon),
      })),
    [shownLegs]
  );

  function countryTally(countryId: string): ExplorerTally | null {
    if (activeYearObj) {
      return activeYearObj.country_tallies[countryId] ?? null;
    }
    const v = visitsById.get(countryId);
    return v ? { count: v.count, distance_m: v.distance_m } : null;
  }

  function cityTally(city: ExplorerCityVisit): ExplorerTally {
    if (activeYearObj) {
      return activeYearObj.city_tallies[String(city.city_id)] ?? { count: 0, distance_m: 0 };
    }
    return { count: city.count, distance_m: city.distance_m };
  }

  function describeTally(t: ExplorerTally): string {
    const prefix = effectiveYear != null ? `${effectiveYear}: ` : "";
    const kmStr = t.distance_m > 0 ? ` · ${Math.round(t.distance_m / 1000).toLocaleString()} km` : "";
    return `${prefix}${t.count.toLocaleString()} ${nounForFilter(sportFilter, t.count)}${kmStr}`;
  }

  function describeCountry(countryId: string, fallbackName: string): string {
    const visit = visitsById.get(countryId);
    const name = visit?.name ?? fallbackName;
    const t = countryTally(countryId);
    if (t && t.count > 0) return `${name} · ${describeTally(t)}`;
    if (effectiveYear != null && visit) return `${name} · none in ${effectiveYear}`;
    return name;
  }

  function describeCity(c: ExplorerCityVisit): string {
    const t = cityTally(c);
    if (t.count > 0) return `${c.name} · ${describeTally(t)}`;
    return c.name;
  }

  function clampPan(nextPan: { x: number; y: number }, nextZoom: number): { x: number; y: number } {
    const el = mapContainerRef.current;
    if (!el || nextZoom <= 1) return { x: 0, y: 0 };
    const rect = el.getBoundingClientRect();
    const maxX = (rect.width * (nextZoom - 1)) / 2 + 40;
    const maxY = (rect.height * (nextZoom - 1)) / 2 + 40;
    return {
      x: Math.max(-maxX, Math.min(maxX, nextPan.x)),
      y: Math.max(-maxY, Math.min(maxY, nextPan.y)),
    };
  }

  function applyZoom(factor: number, clientX?: number, clientY?: number) {
    setZoom((prevZoom) => {
      const nextZoom = Math.max(1, Math.min(12, prevZoom * factor));
      if (nextZoom === 1) {
        setPan({ x: 0, y: 0 });
        return 1;
      }
      const el = mapContainerRef.current;
      if (el && clientX != null && clientY != null) {
        const rect = el.getBoundingClientRect();
        const cx = clientX - rect.left - rect.width / 2;
        const cy = clientY - rect.top - rect.height / 2;
        const ratio = nextZoom / prevZoom;
        setPan((prevPan) =>
          clampPan(
            {
              x: cx - (cx - prevPan.x) * ratio,
              y: cy - (cy - prevPan.y) * ratio,
            },
            nextZoom
          )
        );
      } else {
        setPan((prevPan) => clampPan(prevPan, nextZoom));
      }
      return nextZoom;
    });
  }

  function resetView() {
    setZoom(1);
    setPan({ x: 0, y: 0 });
  }

  const heroCountries = activeYearObj
    ? activeYearObj.country_ids.length
    : summary?.visits.length ?? 0;
  const heroCities = activeYearObj
    ? activeYearObj.city_ids.length
    : summary?.cities.length ?? 0;
  const heroContinents = useMemo(() => {
    if (!summary) return 0;
    if (!activeYearObj) return summary.continents.length;
    const set = new Set<string>();
    for (const id of activeYearObj.country_ids) {
      const v = visitsById.get(id);
      if (v?.continent) set.add(v.continent);
    }
    return set.size;
  }, [summary, activeYearObj, visitsById]);

  const chartYearsAsc = useMemo(() => {
    if (!summary) return [];
    return [...summary.years].sort((a, b) => a.year - b.year);
  }, [summary]);

  const sortedCities = useMemo(() => {
    return [...shownCities].sort((a, b) => {
      const ta = cityTally(a);
      const tb = cityTally(b);
      if (tb.count !== ta.count) return tb.count - ta.count;
      return tb.distance_m - ta.distance_m;
    });
  }, [shownCities, activeYearObj]);

  const displayedCities = allCitiesShown ? sortedCities : sortedCities.slice(0, 10);

  const sortedCountries = useMemo(() => {
    return [...shownCountries].sort((a, b) => {
      const ta = countryTally(a.id) ?? { count: 0, distance_m: 0 };
      const tb = countryTally(b.id) ?? { count: 0, distance_m: 0 };
      if (tb.count !== ta.count) return tb.count - ta.count;
      return tb.distance_m - ta.distance_m;
    });
  }, [shownCountries, activeYearObj]);

  const showCityDots = zoom >= 2.8;
  const markerInvScale = 1 / Math.sqrt(zoom);

  return (
    <div className="explorer-root">
      {/* Top Bar */}
      <div className="page-header" style={{ marginBottom: 12 }}>
        <div className="page-header-left">
          <h2 className="today-title">Explorer</h2>
          <span className="today-subtitle">
            {summary
              ? `${summary.visits.length} ${summary.visits.length === 1 ? "country" : "countries"} · ${summary.total_located_activities.toLocaleString()} GPS activities`
              : loading
              ? "Resolving countries and cities…"
              : "World travel map"}
          </span>
        </div>

        <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
          <div className="segmented-control" role="tablist" aria-label="Explorer sport filter">
            {(
              [
                { id: "run", label: "Running" },
                { id: "ride", label: "Cycling" },
                { id: "all", label: "Everything" },
              ] as { id: ExplorerSportFilter; label: string }[]
            ).map((opt) => (
              <button
                key={opt.id}
                type="button"
                className={`seg-btn ${sportFilter === opt.id ? "active" : ""}`}
                onClick={() => {
                  setSelectedYear(null);
                  onChangeSportFilter(opt.id);
                }}
              >
                {opt.label}
              </button>
            ))}
          </div>

          {hasGarminKey && (
            <button
              type="button"
              className={lifetimeBackfillCompleted ? "secondary" : "primary"}
              disabled={backfillBusy}
              onClick={onBackfillHistory}
              style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12.5 }}
              title="Download full Garmin lifetime activity history & GPS coordinates"
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
            style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12.5 }}
          >
            <RefreshCw size={14} />
            {loading ? "Loading…" : "Refresh"}
          </button>
        </div>
      </div>

      {error && (
        <div className="key-status-err" style={{ marginBottom: 10 }}>
          {error}
        </div>
      )}

      {/* Main Split: World Map + Summary Sidebar */}
      <div className="explorer-layout">
        {/* Left/Center Map Pane */}
        <div
          ref={mapContainerRef}
          className={`explorer-map-pane ${dragging ? "dragging" : ""}`}
          onWheel={(e) => {
            e.preventDefault();
            const factor = e.deltaY < 0 ? 1.25 : 0.8;
            applyZoom(factor, e.clientX, e.clientY);
          }}
          onMouseDown={(e) => {
            if (e.button !== 0) return;
            setDragging(true);
            dragStartRef.current = {
              clientX: e.clientX,
              clientY: e.clientY,
              panX: pan.x,
              panY: pan.y,
            };
          }}
          onMouseMove={(e) => {
            if (dragStartRef.current) {
              const dx = e.clientX - dragStartRef.current.clientX;
              const dy = e.clientY - dragStartRef.current.clientY;
              setPan(
                clampPan(
                  {
                    x: dragStartRef.current.panX + dx,
                    y: dragStartRef.current.panY + dy,
                  },
                  zoom
                )
              );
            }
          }}
          onMouseUp={() => {
            setDragging(false);
            dragStartRef.current = null;
          }}
          onMouseLeave={() => {
            setDragging(false);
            dragStartRef.current = null;
            setTooltip(null);
          }}
        >
          <div
            className="explorer-map-stage"
            style={{
              transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`,
              transformOrigin: "center center",
            }}
          >
            <svg
              viewBox={`0 0 ${MAP_W} ${MAP_H}`}
              preserveAspectRatio="xMidYMid meet"
              style={{ width: "100%", height: "100%", display: "block" }}
            >
              {/* Ocean background */}
              <rect x={0} y={0} width={MAP_W} height={MAP_H} fill="var(--map-ocean)" />

              {/* Subtle Equator & Tropic / Longitude graticules */}
              {[-30, 0, 30, 60].map((lat) => {
                const [, y] = projectLonLat(0, lat);
                return (
                  <line
                    key={`lat-${lat}`}
                    x1={0}
                    y1={y}
                    x2={MAP_W}
                    y2={y}
                    stroke="var(--map-grid)"
                    strokeWidth={0.6 * markerInvScale}
                    strokeDasharray={lat === 0 ? undefined : "3 5"}
                  />
                );
              })}
              {[-120, -60, 0, 60, 120].map((lon) => {
                const [x] = projectLonLat(lon, 0);
                return (
                  <line
                    key={`lon-${lon}`}
                    x1={x}
                    y1={0}
                    x2={x}
                    y2={MAP_H}
                    stroke="var(--map-grid)"
                    strokeWidth={0.6 * markerInvScale}
                    strokeDasharray="3 5"
                  />
                );
              })}

              {/* All Countries */}
              {PREBUILT_COUNTRY_PATHS.map((c) => {
                const visited = shownCountryIds.has(c.id);
                return (
                  <path
                    key={c.id}
                    d={c.d}
                    fill={visited ? sportColor : "var(--map-land)"}
                    fillOpacity={visited ? 0.46 : 1}
                    stroke={visited ? sportColor : "var(--map-border)"}
                    strokeWidth={(visited ? 1.15 : 0.65) * markerInvScale}
                    style={{ transition: "fill 140ms ease, fill-opacity 140ms ease" }}
                    onMouseMove={(e) => {
                      const rect = mapContainerRef.current?.getBoundingClientRect();
                      if (!rect) return;
                      setTooltip({
                        text: describeCountry(c.id, c.name),
                        x: e.clientX - rect.left + 12,
                        y: e.clientY - rect.top + 12,
                      });
                    }}
                  />
                );
              })}

              {/* Geodesic travel arcs */}
              {geodesicPaths.map((arc) => (
                <path
                  key={arc.key}
                  d={arc.d}
                  fill="none"
                  stroke="rgba(255,255,255,0.78)"
                  strokeWidth={1.4 * markerInvScale}
                  strokeLinecap="round"
                  strokeDasharray={`${5 * markerInvScale} ${4 * markerInvScale}`}
                  pointerEvents="none"
                />
              ))}

              {/* City dots when zoomed in */}
              {showCityDots &&
                shownCities.map((city) => {
                  const [cx, cy] = projectLonLat(city.lon, city.lat);
                  const r = 3.8 * markerInvScale;
                  return (
                    <circle
                      key={city.city_id}
                      cx={cx}
                      cy={cy}
                      r={r}
                      fill="#FFFFFF"
                      stroke={sportColor}
                      strokeWidth={1.6 * markerInvScale}
                      onMouseMove={(e) => {
                        e.stopPropagation();
                        const rect = mapContainerRef.current?.getBoundingClientRect();
                        if (!rect) return;
                        setTooltip({
                          text: describeCity(city),
                          x: e.clientX - rect.left + 12,
                          y: e.clientY - rect.top + 12,
                        });
                      }}
                    />
                  );
                })}

              {/* Country flag markers */}
              {shownCountries.map((v) => {
                const [mx, my] = projectLonLat(v.label_lon, v.label_lat);
                return (
                  <g
                    key={`marker-${v.id}`}
                    transform={`translate(${mx}, ${my}) scale(${markerInvScale})`}
                    style={{ cursor: "pointer" }}
                    onMouseMove={(e) => {
                      e.stopPropagation();
                      const rect = mapContainerRef.current?.getBoundingClientRect();
                      if (!rect) return;
                      setTooltip({
                        text: describeCountry(v.id, v.name),
                        x: e.clientX - rect.left + 12,
                        y: e.clientY - rect.top + 12,
                      });
                    }}
                  >
                    <circle
                      cx={0}
                      cy={0}
                      r={12}
                      fill="rgba(8, 13, 18, 0.72)"
                      stroke="rgba(255,255,255,0.2)"
                      strokeWidth={0.9}
                    />
                    <g transform="translate(-8.5, -6)">
                      <CountryFlag iso={v.iso} name={v.name} width={17} height={12} />
                    </g>
                  </g>
                );
              })}
            </svg>
          </div>

          {/* Floating Zoom Controls */}
          <div className="explorer-map-controls" onMouseDown={(e) => e.stopPropagation()}>
            <button
              type="button"
              className="explorer-ctrl-btn"
              title="Zoom in"
              aria-label="Zoom in"
              onClick={() => applyZoom(1.45)}
            >
              <Plus size={14} />
            </button>
            <button
              type="button"
              className="explorer-ctrl-btn"
              title="Zoom out"
              aria-label="Zoom out"
              onClick={() => applyZoom(1 / 1.45)}
            >
              <Minus size={14} />
            </button>
            <button
              type="button"
              className="explorer-ctrl-btn"
              title="Reset zoom & pan"
              aria-label="Reset zoom & pan"
              onClick={resetView}
            >
              <RotateCcw size={13} />
            </button>
          </div>

          {/* Hover Tooltip */}
          {tooltip && (
            <div
              className="explorer-tooltip"
              style={{
                left: Math.min(tooltip.x, (mapContainerRef.current?.clientWidth ?? 600) - 210),
                top: Math.max(8, tooltip.y - 6),
              }}
            >
              {tooltip.text}
            </div>
          )}

          {/* Empty Map Overlay */}
          {!loading && summary && summary.visits.length === 0 && (
            <div className="explorer-empty-overlay">
              <Compass size={34} color="var(--accent-primary)" />
              <div style={{ fontWeight: 700, fontSize: 16 }}>No places yet</div>
              <div className="muted" style={{ maxWidth: 320, textAlign: "center" }}>
                {summary.total_activities_in_filter > 0
                  ? `${summary.total_activities_in_filter} ${nounForFilter(sportFilter, summary.total_activities_in_filter)} found, but none have start GPS coordinates yet.`
                  : "Outdoor activities with GPS show up here once they've synced."}
              </div>
              {hasGarminKey && (
                <button
                  type="button"
                  className="primary"
                  disabled={backfillBusy}
                  onClick={onBackfillHistory}
                  style={{ marginTop: 4 }}
                >
                  {backfillBusy ? "Backfilling History…" : "Run Full History Backfill"}
                </button>
              )}
            </div>
          )}
        </div>

        {/* Right Summary Sidebar */}
        <aside className="explorer-sidebar">
          {/* Hero Header */}
          <div style={{ marginBottom: 18 }}>
            <div style={{ display: "flex", alignItems: "baseline", gap: 8, flexWrap: "wrap" }}>
              <span style={{ fontSize: 48, fontWeight: 800, lineHeight: 1, fontVariantNumeric: "tabular-nums" }}>
                {heroCountries}
              </span>
              <span style={{ fontSize: 20, fontWeight: 600 }}>
                {heroCountries === 1 ? "country" : "countries"}
                {effectiveYear != null ? ` in ${effectiveYear}` : ""}
              </span>
            </div>
            <div className="muted" style={{ marginTop: 6, fontSize: 13 }}>
              {heroCities} {heroCities === 1 ? "city" : "cities"} · {heroContinents}{" "}
              {heroContinents === 1 ? "continent" : "continents"}
              {activeYearObj
                ? ` · ${activeYearObj.new_country_ids.length} new ${
                    activeYearObj.new_country_ids.length === 1 ? "country" : "countries"
                  }`
                : ` · ${(summary?.legs.length ?? 0).toLocaleString()} border crossings`}
            </div>
            {effectiveYear == null && summary?.first_visit ? (
              <div
                className="muted"
                style={{
                  marginTop: 6,
                  fontSize: 12.5,
                  display: "flex",
                  alignItems: "center",
                  gap: 6,
                  flexWrap: "wrap",
                }}
              >
                <span>Started in</span>
                <CountryFlag
                  iso={summary.first_visit.iso}
                  name={summary.first_visit.name}
                  width={16}
                  height={11}
                />
                <span>
                  {summary.first_visit.name}, {formatMonthYear(summary.first_visit.first_iso)}
                </span>
              </div>
            ) : effectiveYear != null ? (
              <button
                type="button"
                className="secondary"
                style={{ marginTop: 8, padding: "4px 10px", fontSize: 12 }}
                onClick={() => setSelectedYear(null)}
              >
                Show all years
              </button>
            ) : null}
          </div>

          {/* Countries per year bar chart */}
          {chartYearsAsc.length > 0 && (
            <div style={{ marginBottom: 20 }}>
              <div style={{ fontWeight: 700, fontSize: 14, marginBottom: 8 }}>Countries per year</div>
              {(() => {
                const maxC = Math.max(1, ...chartYearsAsc.map((y) => y.country_ids.length));
                const n = chartYearsAsc.length;
                const svgW = Math.max(290, n * 34);
                const svgH = 128;
                const barW = Math.min(24, Math.max(12, Math.floor((svgW - 16) / n) - 6));
                const slotW = (svgW - 12) / n;
                return (
                  <div style={{ overflowX: "auto" }}>
                    <svg viewBox={`0 0 ${svgW} ${svgH}`} style={{ width: "100%", height: 128, display: "block" }}>
                      {chartYearsAsc.map((y, idx) => {
                        const count = y.country_ids.length;
                        const h = Math.max(4, Math.round((count / maxC) * 76));
                        const x = 6 + idx * slotW + (slotW - barW) / 2;
                        const topY = 96 - h;
                        const active = effectiveYear == null || effectiveYear === y.year;
                        return (
                          <g
                            key={y.year}
                            style={{ cursor: "pointer" }}
                            onClick={() =>
                              setSelectedYear((cur) => (cur === y.year ? null : y.year))
                            }
                          >
                            <rect
                              x={x}
                              y={topY}
                              width={barW}
                              height={h}
                              rx={4}
                              fill={sportColor}
                              fillOpacity={active ? 0.92 : 0.28}
                            />
                            <text
                              x={x + barW / 2}
                              y={topY - 5}
                              textAnchor="middle"
                              fontSize="10.5"
                              fontWeight="700"
                              fill="var(--text)"
                              opacity={active ? 1 : 0.45}
                            >
                              {count}
                            </text>
                            <text
                              x={x + barW / 2}
                              y={114}
                              textAnchor="middle"
                              fontSize="10"
                              fill="var(--muted)"
                            >
                              {String(y.year).slice(-2)}
                            </text>
                          </g>
                        );
                      })}
                    </svg>
                  </div>
                );
              })()}
            </div>
          )}

          {/* By year */}
          {summary && summary.years.length > 0 && (
            <div style={{ marginBottom: 20 }}>
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                  marginBottom: 8,
                }}
              >
                <span style={{ fontWeight: 700, fontSize: 14 }}>By year</span>
                {effectiveYear != null && (
                  <button
                    type="button"
                    className="secondary"
                    style={{ padding: "2px 8px", fontSize: 11.5 }}
                    onClick={() => setSelectedYear(null)}
                  >
                    Show all
                  </button>
                )}
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {summary.years.map((y) => {
                  const selected = effectiveYear === y.year;
                  return (
                    <button
                      key={y.year}
                      type="button"
                      className={`explorer-year-card ${selected ? "selected" : ""}`}
                      style={
                        selected
                          ? { borderColor: sportColor, background: "var(--accent-soft-bg)" }
                          : undefined
                      }
                      onClick={() => setSelectedYear((cur) => (cur === y.year ? null : y.year))}
                    >
                      <div
                        style={{
                          display: "flex",
                          alignItems: "center",
                          justifyContent: "space-between",
                          gap: 8,
                          width: "100%",
                        }}
                      >
                        <span style={{ fontSize: 16, fontWeight: 800, fontVariantNumeric: "tabular-nums" }}>
                          {y.year}
                        </span>
                        <div style={{ display: "flex", alignItems: "center", gap: 6, marginLeft: "auto" }}>
                          <span style={{ fontSize: 12.5, fontWeight: 600 }}>
                            {y.country_ids.length} {y.country_ids.length === 1 ? "country" : "countries"} ·{" "}
                            {y.city_ids.length} {y.city_ids.length === 1 ? "city" : "cities"}
                          </span>
                          {y.new_country_ids.length > 0 && (
                            <span
                              className="pill green"
                              style={{ fontSize: 10.5, padding: "1px 7px" }}
                            >
                              +{y.new_country_ids.length} new
                            </span>
                          )}
                        </div>
                      </div>
                      <div
                        style={{
                          display: "flex",
                          flexWrap: "wrap",
                          gap: 5,
                          marginTop: 6,
                        }}
                        title={y.country_ids
                          .map((id) => visitsById.get(id)?.name ?? id)
                          .join(", ")}
                      >
                        {y.country_ids.map((id) => {
                          const v = visitsById.get(id);
                          return (
                            <CountryFlag
                              key={id}
                              iso={v?.iso ?? id}
                              name={v?.name ?? id}
                              width={20}
                              height={14}
                            />
                          );
                        })}
                      </div>
                    </button>
                  );
                })}
              </div>
            </div>
          )}

          {/* Top Cities */}
          {sortedCities.length > 0 && (
            <div style={{ marginBottom: 20 }}>
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                  marginBottom: 8,
                }}
              >
                <span style={{ fontWeight: 700, fontSize: 14 }}>
                  {effectiveYear != null
                    ? `Cities in ${effectiveYear}`
                    : `Cities, most ${nounForFilter(sportFilter, 2)} first`}
                </span>
                <span className="muted" style={{ fontWeight: 700 }}>
                  {sortedCities.length}
                </span>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 7 }}>
                {displayedCities.map((c, idx) => {
                  const t = cityTally(c);
                  return (
                    <div key={c.city_id} className="explorer-rank-row">
                      <span className="explorer-rank-idx">{idx + 1}</span>
                      <CountryFlag iso={c.country_iso} name={c.country_name} width={18} height={13} />
                      <div style={{ minWidth: 0, flex: 1 }}>
                        <div style={{ fontSize: 13, fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                          {c.name}
                        </div>
                        <div className="muted" style={{ fontSize: 11.5 }}>
                          First {formatDayMonthYear(c.first_iso)}
                        </div>
                      </div>
                      <div style={{ textAlign: "right", flexShrink: 0 }}>
                        <div style={{ fontSize: 13, fontWeight: 600, fontVariantNumeric: "tabular-nums" }}>
                          {t.count}×
                        </div>
                        {t.distance_m > 0 && (
                          <div className="muted" style={{ fontSize: 11.5, fontVariantNumeric: "tabular-nums" }}>
                            {Math.round(t.distance_m / 1000).toLocaleString()} km
                          </div>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
              {sortedCities.length > 10 && (
                <button
                  type="button"
                  className="secondary"
                  style={{ marginTop: 8, padding: "4px 10px", fontSize: 12 }}
                  onClick={() => setAllCitiesShown((v) => !v)}
                >
                  {allCitiesShown ? "Show top 10" : `Show all ${sortedCities.length}`}
                </button>
              )}
              <div className="muted" style={{ fontSize: 11, marginTop: 8, lineHeight: 1.4 }}>
                City data © GeoNames (CC BY 4.0): towns of 15,000 people or more, within 25 km of where an activity started.
              </div>
            </div>
          )}

          {/* Every Country */}
          {sortedCountries.length > 0 && (
            <div>
              <div style={{ fontWeight: 700, fontSize: 14, marginBottom: 8 }}>
                {effectiveYear != null
                  ? `Countries in ${effectiveYear}, most ${nounForFilter(sportFilter, 2)} first`
                  : `Every country, most ${nounForFilter(sportFilter, 2)} first`}
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 7 }}>
                {sortedCountries.map((v, idx) => {
                  const t = countryTally(v.id) ?? { count: v.count, distance_m: v.distance_m };
                  const isNewInYear = Boolean(
                    activeYearObj && activeYearObj.new_country_ids.includes(v.id)
                  );
                  const cityCount = shownCities.filter((c) => c.country_id === v.id).length;
                  return (
                    <div key={v.id} className="explorer-rank-row">
                      <span className="explorer-rank-idx">{idx + 1}</span>
                      <CountryFlag iso={v.iso} name={v.name} width={18} height={13} />
                      <div style={{ minWidth: 0, flex: 1 }}>
                        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                          <span style={{ fontSize: 13, fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                            {v.name}
                          </span>
                          {isNewInYear && (
                            <span className="pill green" style={{ fontSize: 10, padding: "1px 6px" }}>
                              New
                            </span>
                          )}
                        </div>
                        <div className="muted" style={{ fontSize: 11.5 }}>
                          First {formatDayMonthYear(v.first_iso)}
                        </div>
                      </div>
                      <div style={{ textAlign: "right", flexShrink: 0 }}>
                        <div style={{ fontSize: 13, fontWeight: 600, fontVariantNumeric: "tabular-nums" }}>
                          {t.count}×
                        </div>
                        <div className="muted" style={{ fontSize: 11.5, fontVariantNumeric: "tabular-nums" }}>
                          {cityCount} {cityCount === 1 ? "city" : "cities"}
                        </div>
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}
