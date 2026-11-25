use crate::models::{
    ActivityRecord, ExplorerCityVisit, ExplorerCountryVisit, ExplorerFirstVisit, ExplorerLeg,
    ExplorerSummaryPayload, ExplorerTally, ExplorerYearSummary,
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const COUNTRIES_JSON: &str = include_str!("../data/countries.json");
const CITIES_JSON: &str = include_str!("../data/cities.json");

#[derive(Debug, Clone)]
struct BBox {
    min_lat: f64,
    max_lat: f64,
    min_lon: f64,
    max_lon: f64,
}

#[derive(Debug, Clone)]
pub struct Country {
    pub id: String,
    pub iso: String,
    pub name: String,
    pub continent: String,
    pub flag: String,
    pub label_lat: f64,
    pub label_lon: f64,
    rings: Vec<Vec<(f64, f64)>>,
    boxes: Vec<BBox>,
}

impl Country {
    fn ring_area(&self, i: usize) -> f64 {
        let b = &self.boxes[i];
        (b.max_lat - b.min_lat) * (b.max_lon - b.min_lon)
    }

    fn contains(&self, lat: f64, lon: f64) -> Option<usize> {
        for (i, b) in self.boxes.iter().enumerate() {
            if lat >= b.min_lat
                && lat <= b.max_lat
                && lon >= b.min_lon
                && lon <= b.max_lon
                && point_in_ring(&self.rings[i], lat, lon)
            {
                return Some(i);
            }
        }
        None
    }
}

fn point_in_ring(ring: &[(f64, f64)], lat: f64, lon: f64) -> bool {
    if ring.is_empty() {
        return false;
    }
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a_lon, a_lat) = ring[i];
        let (b_lon, b_lat) = ring[j];
        if (a_lat > lat) != (b_lat > lat)
            && lon < (b_lon - a_lon) * (lat - a_lat) / (b_lat - a_lat) + a_lon
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn iso_to_flag_emoji(iso: &str) -> String {
    let bytes = iso.as_bytes();
    if bytes.len() == 2 && bytes.iter().all(|b| b.is_ascii_alphabetic()) {
        let mut s = String::with_capacity(8);
        for &b in bytes {
            let upper = b.to_ascii_uppercase() as u32;
            if let Some(ch) = char::from_u32(127_397 + upper) {
                s.push(ch);
            }
        }
        if !s.is_empty() {
            return s;
        }
    }
    "🏳️".to_string()
}

#[derive(Deserialize)]
struct RawCountry {
    iso: String,
    name: String,
    continent: String,
    label: Vec<f64>,
    rings: Vec<Vec<f64>>,
}

pub struct Atlas {
    countries: Vec<Country>,
}

impl Atlas {
    pub fn shared() -> &'static Self {
        static INSTANCE: OnceLock<Atlas> = OnceLock::new();
        INSTANCE.get_or_init(|| Self::from_json(COUNTRIES_JSON))
    }

    fn from_json(raw_str: &str) -> Self {
        let raw: Vec<RawCountry> = serde_json::from_str(raw_str).unwrap_or_default();
        let mut countries = Vec::with_capacity(raw.len());
        for r in raw {
            if r.label.len() < 2 {
                continue;
            }
            let mut rings = Vec::new();
            let mut boxes = Vec::new();
            for flat in r.rings {
                if flat.len() < 6 {
                    continue;
                }
                let mut pts = Vec::with_capacity(flat.len() / 2);
                let mut min_lat = f64::INFINITY;
                let mut max_lat = f64::NEG_INFINITY;
                let mut min_lon = f64::INFINITY;
                let mut max_lon = f64::NEG_INFINITY;
                for pair in flat.chunks_exact(2) {
                    let lon = pair[0];
                    let lat = pair[1];
                    min_lat = min_lat.min(lat);
                    max_lat = max_lat.max(lat);
                    min_lon = min_lon.min(lon);
                    max_lon = max_lon.max(lon);
                    pts.push((lon, lat));
                }
                if !pts.is_empty() {
                    rings.push(pts);
                    boxes.push(BBox {
                        min_lat,
                        max_lat,
                        min_lon,
                        max_lon,
                    });
                }
            }
            if rings.is_empty() {
                continue;
            }
            let id = if r.iso == "-99" {
                r.name.clone()
            } else {
                r.iso.clone()
            };
            let flag = iso_to_flag_emoji(&r.iso);
            countries.push(Country {
                id,
                iso: r.iso,
                name: r.name,
                continent: r.continent,
                flag,
                label_lon: r.label[0],
                label_lat: r.label[1],
                rings,
                boxes,
            });
        }
        Self { countries }
    }

    pub fn country(&self, lat: f64, lon: f64) -> Option<&Country> {
        let mut best: Option<(&Country, f64)> = None;
        for c in &self.countries {
            if let Some(i) = c.contains(lat, lon) {
                let area = c.ring_area(i);
                if best.map_or(true, |(_, best_area)| area < best_area) {
                    best = Some((c, area));
                }
            }
        }
        if let Some((c, _)) = best {
            return Some(c);
        }

        let k = (lat * std::f64::consts::PI / 180.0).cos().abs().max(0.05);
        let margin = 0.25;
        let lon_margin = margin / k;
        let mut nearest: Option<(&Country, f64)> = None;
        for c in &self.countries {
            for (i, b) in c.boxes.iter().enumerate() {
                if lat >= b.min_lat - margin
                    && lat <= b.max_lat + margin
                    && lon >= b.min_lon - lon_margin
                    && lon <= b.max_lon + lon_margin
                {
                    for &(p_lon, p_lat) in &c.rings[i] {
                        let d_lat = p_lat - lat;
                        let d_lon = (p_lon - lon) * k;
                        let d2 = d_lat * d_lat + d_lon * d_lon;
                        if nearest.map_or(true, |(_, bd)| d2 < bd) {
                            nearest = Some((c, d2));
                        }
                    }
                }
            }
        }
        if let Some((c, d2)) = nearest {
            if d2 < 0.225 * 0.225 {
                return Some(c);
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
pub struct City {
    pub id: usize,
    pub name: String,
    pub iso: String,
    pub lat: f64,
    pub lon: f64,
    pub population: i64,
    pub admin1: String,
    pub capital: bool,
}

pub struct CityAtlas {
    cities: Vec<City>,
    megacities: Vec<usize>,
    grid: HashMap<i32, Vec<usize>>,
}

impl CityAtlas {
    const MAX_KM: f64 = 25.0;

    pub fn shared() -> &'static Self {
        static INSTANCE: OnceLock<CityAtlas> = OnceLock::new();
        INSTANCE.get_or_init(|| Self::from_json(CITIES_JSON))
    }

    fn cell(lat: f64, lon: f64) -> i32 {
        let lat_cell = (lat + 90.0).floor() as i32;
        let lon_cell = ((lon + 180.0).floor() as i32).rem_euclid(360);
        lat_cell * 1000 + lon_cell
    }

    fn reach(c: &City) -> f64 {
        (5.0 * ((c.population.max(0) as f64) / 100_000.0).sqrt()).min(12.0)
    }

    fn from_json(raw_str: &str) -> Self {
        let rows: Vec<Vec<serde_json::Value>> =
            serde_json::from_str(raw_str).unwrap_or_default();
        let mut cities = Vec::with_capacity(rows.len());
        let mut megacities = Vec::new();
        let mut grid: HashMap<i32, Vec<usize>> = HashMap::new();

        for r in rows {
            if r.len() != 7 {
                continue;
            }
            let Some(name) = r[0].as_str().map(str::to_string) else {
                continue;
            };
            let Some(iso) = r[1].as_str().map(str::to_string) else {
                continue;
            };
            let Some(lat) = r[2].as_f64() else {
                continue;
            };
            let Some(lon) = r[3].as_f64() else {
                continue;
            };
            let Some(population) = r[4].as_i64().or_else(|| r[4].as_f64().map(|v| v as i64)) else {
                continue;
            };
            let Some(admin1) = r[5].as_str().map(str::to_string) else {
                continue;
            };
            let capital = r[6]
                .as_bool()
                .or_else(|| r[6].as_i64().map(|v| v != 0))
                .unwrap_or(false);

            let id = cities.len();
            let key = Self::cell(lat, lon);
            grid.entry(key).or_default().push(id);
            if capital && population >= 3_000_000 {
                megacities.push(id);
            }
            cities.push(City {
                id,
                name,
                iso,
                lat,
                lon,
                population,
                admin1,
                capital,
            });
        }

        Self {
            cities,
            megacities,
            grid,
        }
    }

    pub fn city(&self, lat: f64, lon: f64, iso: Option<&str>) -> Option<&City> {
        let found = self.nearest(lat, lon, iso)?;
        let k = (lat * std::f64::consts::PI / 180.0).cos();
        for &mega_idx in &self.megacities {
            let m = &self.cities[mega_idx];
            if m.iso == found.iso && m.admin1 == found.admin1 && m.id != found.id {
                let km = (m.lat - lat).hypot((m.lon - lon) * k) * 111.2;
                if km <= 60.0 {
                    return Some(m);
                }
            }
        }
        Some(found)
    }

    fn nearest(&self, lat: f64, lon: f64, iso: Option<&str>) -> Option<&City> {
        let k = (lat * std::f64::consts::PI / 180.0).cos();
        let mut nearby: Vec<(&City, f64)> = Vec::new();
        for d_lat in -1..=1 {
            for d_lon in -1..=1 {
                let cell_key = Self::cell(lat + d_lat as f64, lon + d_lon as f64);
                if let Some(indices) = self.grid.get(&cell_key) {
                    for &idx in indices {
                        let c = &self.cities[idx];
                        if let Some(code) = iso {
                            if code.len() == 2 && c.iso != code {
                                continue;
                            }
                        }
                        let km = (c.lat - lat).hypot((c.lon - lon) * k) * 111.2;
                        if km <= Self::MAX_KM {
                            nearby.push((c, km));
                        }
                    }
                }
            }
        }
        if nearby.is_empty() {
            return None;
        }
        if let Some((biggest, _)) = nearby
            .iter()
            .filter(|(c, km)| *km <= Self::reach(c))
            .max_by_key(|(c, _)| c.population)
        {
            return Some(*biggest);
        }
        nearby
            .into_iter()
            .min_by(|(ca, ka), (cb, kb)| {
                let sa = ka / (ca.population.max(15_000) as f64).powf(0.3);
                let sb = kb / (cb.population.max(15_000) as f64).powf(0.3);
                sa.total_cmp(&sb)
            })
            .map(|(c, _)| c)
    }
}

pub fn normalize_sport_filter(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ride" | "cycling" => "ride",
        "all" | "everything" => "all",
        _ => "run",
    }
}

fn is_indoor_type_key(type_key: &str) -> bool {
    let lower = type_key.to_ascii_lowercase();
    lower.contains("indoor")
        || lower.contains("virtual")
        || lower.contains("treadmill")
        || lower.contains("trainer")
}

fn sport_matches(type_key: &str, sport_filter: &str) -> bool {
    let lower = type_key.to_ascii_lowercase();
    match sport_filter {
        "run" => lower.contains("run"),
        "ride" => {
            lower.contains("cycl")
                || lower.contains("bik")
                || lower.contains("ride")
                || lower.contains("mountain")
        }
        _ => true,
    }
}

pub fn compute_explorer_summary(
    activities: &[ActivityRecord],
    raw_filter: &str,
) -> ExplorerSummaryPayload {
    let sport_filter = normalize_sport_filter(raw_filter);
    let atlas = Atlas::shared();
    let city_atlas = CityAtlas::shared();

    let mut total_activities_in_filter = 0usize;
    let mut located_points: Vec<&ActivityRecord> = Vec::new();
    for a in activities {
        if !sport_matches(&a.type_key, sport_filter) || is_indoor_type_key(&a.type_key) {
            continue;
        }
        total_activities_in_filter += 1;
        let Some(lat) = a.start_latitude else {
            continue;
        };
        let Some(lon) = a.start_longitude else {
            continue;
        };
        if !lat.is_finite()
            || !lon.is_finite()
            || (lat == 0.0 && lon == 0.0)
            || lat.abs() > 90.0
            || lon.abs() > 180.0
        {
            continue;
        }
        located_points.push(a);
    }

    located_points.sort_by(|a, b| {
        a.start_iso
            .cmp(&b.start_iso)
            .then_with(|| a.garmin_id.cmp(&b.garmin_id))
    });

    let mut visit_map: HashMap<String, ExplorerCountryVisit> = HashMap::new();
    let mut visit_order: Vec<String> = Vec::new();
    let mut city_map: HashMap<usize, ExplorerCityVisit> = HashMap::new();
    let mut city_order: Vec<usize> = Vec::new();
    let mut legs: Vec<ExplorerLeg> = Vec::new();
    let mut previous_country: Option<&Country> = None;

    let mut per_year_countries: HashMap<i32, Vec<String>> = HashMap::new();
    let mut per_year_country_seen: HashMap<i32, HashSet<String>> = HashMap::new();
    let mut first_country_year: HashMap<String, i32> = HashMap::new();
    let mut country_years: HashMap<i32, HashMap<String, ExplorerTally>> = HashMap::new();

    let mut per_year_cities: HashMap<i32, HashSet<usize>> = HashMap::new();
    let mut first_city_year: HashMap<usize, i32> = HashMap::new();
    let mut city_years: HashMap<i32, HashMap<usize, ExplorerTally>> = HashMap::new();
    let mut total_located_activities = 0usize;

    for a in located_points {
        let (Some(lat), Some(lon)) = (a.start_latitude, a.start_longitude) else {
            continue;
        };
        let Some(country) = atlas.country(lat, lon) else {
            continue;
        };
        total_located_activities += 1;
        let dist = a.distance_m.max(0.0);
        let year = a
            .start_iso
            .get(..4)
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(2026);

        if let Some(v) = visit_map.get_mut(&country.id) {
            v.last_iso = a.start_iso.clone();
            v.count += 1;
            v.distance_m += dist;
        } else {
            visit_order.push(country.id.clone());
            visit_map.insert(
                country.id.clone(),
                ExplorerCountryVisit {
                    id: country.id.clone(),
                    iso: country.iso.clone(),
                    name: country.name.clone(),
                    continent: country.continent.clone(),
                    flag: country.flag.clone(),
                    label_lat: country.label_lat,
                    label_lon: country.label_lon,
                    first_iso: a.start_iso.clone(),
                    last_iso: a.start_iso.clone(),
                    count: 1,
                    distance_m: dist,
                },
            );
        }

        if let Some(prev) = previous_country {
            if prev.id != country.id {
                legs.push(ExplorerLeg {
                    from_country_id: prev.id.clone(),
                    from_lat: prev.label_lat,
                    from_lon: prev.label_lon,
                    to_country_id: country.id.clone(),
                    to_lat: country.label_lat,
                    to_lon: country.label_lon,
                    date_iso: a.start_iso.clone(),
                    year,
                });
            }
        }
        previous_country = Some(country);

        let seen = per_year_country_seen.entry(year).or_default();
        if seen.insert(country.id.clone()) {
            per_year_countries
                .entry(year)
                .or_default()
                .push(country.id.clone());
        }
        let ct = country_years
            .entry(year)
            .or_default()
            .entry(country.id.clone())
            .or_default();
        ct.count += 1;
        ct.distance_m += dist;
        first_country_year.entry(country.id.clone()).or_insert(year);

        if let Some(city) = city_atlas.city(lat, lon, Some(&country.iso)) {
            if let Some(cv) = city_map.get_mut(&city.id) {
                cv.count += 1;
                cv.distance_m += dist;
            } else {
                city_order.push(city.id);
                first_city_year.entry(city.id).or_insert(year);
                city_map.insert(
                    city.id,
                    ExplorerCityVisit {
                        city_id: city.id,
                        name: city.name.clone(),
                        country_id: country.id.clone(),
                        country_iso: country.iso.clone(),
                        country_name: country.name.clone(),
                        flag: country.flag.clone(),
                        lat: city.lat,
                        lon: city.lon,
                        first_iso: a.start_iso.clone(),
                        count: 1,
                        distance_m: dist,
                    },
                );
            }
            per_year_cities.entry(year).or_default().insert(city.id);
            let cit_t = city_years
                .entry(year)
                .or_default()
                .entry(city.id)
                .or_default();
            cit_t.count += 1;
            cit_t.distance_m += dist;
        }
    }

    let visits: Vec<ExplorerCountryVisit> = visit_order
        .into_iter()
        .filter_map(|id| visit_map.remove(&id))
        .collect();
    let cities: Vec<ExplorerCityVisit> = city_order
        .into_iter()
        .filter_map(|id| city_map.remove(&id))
        .collect();

    let mut year_keys: Vec<i32> = per_year_countries.keys().copied().collect();
    year_keys.sort_by(|a, b| b.cmp(a));
    let mut years = Vec::with_capacity(year_keys.len());
    for y in year_keys {
        let country_ids = per_year_countries.remove(&y).unwrap_or_default();
        let new_country_ids: Vec<String> = country_ids
            .iter()
            .filter(|cid| first_country_year.get(*cid).copied() == Some(y))
            .cloned()
            .collect();
        let city_set = per_year_cities.remove(&y).unwrap_or_default();
        let new_cities = city_set
            .iter()
            .filter(|cid| first_city_year.get(*cid).copied() == Some(y))
            .count();
        let mut city_ids: Vec<usize> = city_set.into_iter().collect();
        city_ids.sort_unstable();
        years.push(ExplorerYearSummary {
            year: y,
            country_ids,
            new_country_ids,
            city_ids,
            new_cities,
            country_tallies: country_years.remove(&y).unwrap_or_default(),
            city_tallies: city_years.remove(&y).unwrap_or_default(),
        });
    }

    let mut continents_set: HashSet<String> = HashSet::new();
    for v in &visits {
        if !v.continent.trim().is_empty() {
            continents_set.insert(v.continent.clone());
        }
    }
    let mut continents: Vec<String> = continents_set.into_iter().collect();
    continents.sort();

    let first_visit = visits.first().map(|v| ExplorerFirstVisit {
        country_id: v.id.clone(),
        iso: v.iso.clone(),
        name: v.name.clone(),
        flag: v.flag.clone(),
        first_iso: v.first_iso.clone(),
    });

    ExplorerSummaryPayload {
        sport_filter: sport_filter.to_string(),
        visits,
        cities,
        legs,
        years,
        continents,
        first_visit,
        total_located_activities,
        total_activities_in_filter,
    }
}
