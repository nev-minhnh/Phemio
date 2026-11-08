//! Weather + world-clock lookups via Open-Meteo (free, no API key).
//! Geocoding turns a typed place name into coordinates and an IANA time zone;
//! the forecast call (timezone=auto) also returns that zone's UTC offset,
//! which is how the second clock gets its time.

use std::sync::Mutex;

use serde_json::Value;
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::net;
use crate::util;

#[derive(Clone, Debug, Default)]
pub struct Geo {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Day {
    pub day: i64, // days since 1970 (date at the location)
    pub code: i32,
    pub max: f32,
    pub min: f32,
    /// Chance of rain in %, or -1 when the source doesn't give one.
    pub rain: i32,
    /// Precipitation total in mm (-1 unknown).
    pub rain_mm: f32,
}

/// One hour (or a 3-hour step for wttr.in) of today's and tomorrow's forecast.
#[derive(Clone, Debug, Default)]
pub struct Hour {
    /// Wall-clock seconds at the place (days * 86400 + seconds of day).
    pub t: i64,
    pub temp: f32,
    pub feels: f32,
    pub humidity: i32,
    /// Chance of rain % (-1 unknown).
    pub rain: i32,
    /// Rain in mm for this step (-1 unknown).
    pub mm: f32,
    pub code: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Weather {
    /// Today and tomorrow, hour by hour (from midnight today at the place).
    pub hours: Vec<Hour>,
    pub temp: f32,
    pub feels: f32,
    pub humidity: i32,
    pub wind: f32,
    pub code: i32,
    pub is_day: bool,
    pub days: Vec<Day>,
    pub offset: i64,
    pub abbr: String,
    /// The place's time zone (IANA name), when the source says.
    pub zone: String,
    pub fetched: i64,
    pub unit_f: bool,
    pub provider: i32,
}

/// Forecast sources (Settings → General → Forecast source). All free, no key.
pub const PROVIDERS: [&str; 5] = ["Open-Meteo", "MET Norway", "wttr.in", "ECMWF", "NOAA GFS"];

#[derive(Clone, Debug)]
pub enum Kind {
    Weather,
    Tz,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub kind: Kind,
    /// The query this answers (so stale answers can be ignored).
    pub city: String,
    pub geo: Option<Geo>,
    pub weather: Option<Weather>,
    pub error: Option<String>,
}

static RESULTS: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());

pub fn take_results() -> Vec<Outcome> {
    RESULTS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}

/// Look up (and geocode if `known` is None) on a worker thread; posts `msg` when done.
pub fn spawn(kind: Kind, city: String, known: Option<(f64, f64)>, unit_f: bool, provider: i32, hwnd: HWND, msg: u32) {
    let h = hwnd.0 as isize;
    let _ = std::thread::Builder::new().stack_size(512 * 1024).spawn(move || {
        let mut out = Outcome { kind: kind.clone(), city: city.clone(), geo: None, weather: None, error: None };
        let coords = match known {
            Some(c) => Ok(c),
            None => geocode(&city).map(|g| {
                let c = (g.lat, g.lon);
                out.geo = Some(g);
                c
            }),
        };
        let provider = if matches!(kind, Kind::Tz) { 0 } else { provider };
        match coords.and_then(|(lat, lon)| forecast(lat, lon, unit_f, matches!(kind, Kind::Tz), provider)) {
            Ok(w) => out.weather = Some(w),
            Err(e) => out.error = Some(e),
        }
        if let Ok(mut v) = RESULTS.lock() {
            v.push(out);
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    });
}

/// The error when no place matches the city name.
pub const NOT_FOUND: &str = "Not found";

/// Find a place by its name only (never this PC's location).
pub fn geocode(q: &str) -> Result<Geo, String> {
    let q = q.trim();
    if q.is_empty() {
        return Err("No location set".into());
    }
    // Accept "lat,lon" directly.
    if let Some((a, b)) = q.split_once(',') {
        if let (Ok(lat), Ok(lon)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
            return Ok(Geo { name: format!("{lat:.2}, {lon:.2}"), lat, lon });
        }
    }
    // "Melbourne, Australia" → search the first part, prefer a matching country.
    let (name, hint) = match q.split_once(',') {
        Some((n, h)) => (n.trim(), h.trim().to_lowercase()),
        None => (q, String::new()),
    };
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=8&language=en&format=json",
        util::url_encode(name)
    );
    let v = net::get_json(&url)?;
    parse_geocode(&v, &hint).ok_or_else(|| NOT_FOUND.to_string())
}

pub fn parse_geocode(v: &Value, hint: &str) -> Option<Geo> {
    let arr = v.get("results")?.as_array()?;
    let pick = arr
        .iter()
        .find(|r| {
            !hint.is_empty()
                && [r.get("country"), r.get("admin1"), r.get("country_code")]
                    .iter()
                    .flatten()
                    .any(|s| s.as_str().map(|s| s.to_lowercase().starts_with(hint)).unwrap_or(false))
        })
        .or_else(|| arr.first())?;
    let name = pick.get("name")?.as_str()?.to_string();
    let country = pick.get("country").and_then(|c| c.as_str()).unwrap_or("");
    Some(Geo {
        name: if country.is_empty() { name } else { format!("{name}, {country}") },
        lat: pick.get("latitude")?.as_f64()?,
        lon: pick.get("longitude")?.as_f64()?,
    })
}

pub fn forecast(lat: f64, lon: f64, unit_f: bool, tz_only: bool, provider: i32) -> Result<Weather, String> {
    let mut w = match provider {
        1 => met_norway(lat, lon, unit_f)?,
        2 => wttr(lat, lon, unit_f)?,
        _ => open_meteo(lat, lon, unit_f, tz_only, provider)?,
    };
    w.unit_f = unit_f;
    w.provider = provider;
    w.fetched = util::unix_now();
    Ok(w)
}

fn open_meteo(lat: f64, lon: f64, unit_f: bool, tz_only: bool, provider: i32) -> Result<Weather, String> {
    let unit = if unit_f { "&temperature_unit=fahrenheit&wind_speed_unit=mph" } else { "" };
    let model = match provider {
        3 => "&models=ecmwf_ifs025",
        4 => "&models=gfs_seamless",
        _ => "",
    };
    let url = if tz_only {
        format!("https://api.open-meteo.com/v1/forecast?latitude={lat:.4}&longitude={lon:.4}&current=temperature_2m,weather_code,is_day&forecast_days=1&timezone=auto{unit}")
    } else {
        format!(
            "https://api.open-meteo.com/v1/forecast?latitude={lat:.4}&longitude={lon:.4}\
             &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,is_day,wind_speed_10m\
             &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max,precipitation_sum\
             &hourly=temperature_2m,apparent_temperature,relative_humidity_2m,precipitation_probability,precipitation,weather_code\
             &forecast_days=7&timezone=auto{unit}{model}"
        )
    };
    let v = net::get_json(&url)?;
    parse_forecast(&v).ok_or_else(|| "Unexpected weather data".to_string())
}

// ---------------------------------------------------------------------------
// MET Norway (yr.no) — Locationforecast 2.0 compact
// ---------------------------------------------------------------------------

fn met_norway(lat: f64, lon: f64, unit_f: bool) -> Result<Weather, String> {
    let url = format!("https://api.met.no/weatherapi/locationforecast/2.0/compact?lat={lat:.4}&lon={lon:.4}");
    let v = net::get_json_with(&url, &[("User-Agent", "Phemio/0.7 (Windows dynamic island app)")])?;
    // MET gives UTC times; the place's UTC offset comes from a tiny Open-Meteo call.
    let offset = open_meteo(lat, lon, false, true, 0).map(|w| w.offset).unwrap_or(((lon / 15.0).round() * 3600.0) as i64);
    parse_met(&v, offset, unit_f).ok_or_else(|| "Unexpected MET Norway data".to_string())
}

/// "2026-10-26T13:00:00Z" → unix seconds.
fn parse_iso_utc(t: &str) -> Option<i64> {
    let (d, rest) = t.split_once('T')?;
    let p: Vec<i64> = d.split('-').filter_map(|x| x.parse().ok()).collect();
    let hm: Vec<i64> = rest.trim_end_matches('Z').split(':').filter_map(|x| x.parse().ok()).collect();
    if p.len() != 3 || hm.len() < 2 {
        return None;
    }
    Some(util::days_from_civil(p[0], p[1], p[2]) * 86400 + hm[0] * 3600 + hm[1] * 60)
}

/// MET symbol ("lightrainshowers_day") → (WMO code, is_day).
pub fn met_symbol(sym: &str) -> (i32, bool) {
    let night = sym.ends_with("_night");
    let base = sym.split('_').next().unwrap_or(sym);
    let code = if base.contains("thunder") {
        95
    } else if base.contains("snow") {
        if base.contains("showers") { 85 } else if base.starts_with("heavy") { 75 } else if base.starts_with("light") { 71 } else { 73 }
    } else if base.contains("sleet") {
        66
    } else if base.contains("rainshowers") {
        if base.starts_with("heavy") { 82 } else { 80 }
    } else if base.contains("rain") {
        if base.starts_with("heavy") { 65 } else if base.starts_with("light") { 61 } else { 63 }
    } else {
        match base {
            "clearsky" => 0,
            "fair" => 1,
            "partlycloudy" => 2,
            "cloudy" => 3,
            "fog" => 45,
            _ => 3,
        }
    };
    (code, !night)
}

/// How "bad" a WMO code is — the day's icon is its worst daytime weather.
fn severity(code: i32) -> i32 {
    match code {
        95..=99 => 9,
        71..=77 | 85 | 86 => 8,
        65 | 82 => 7,
        66 | 67 | 56 | 57 => 6,
        63 | 81 => 5,
        61 | 80 => 4,
        51..=55 => 3,
        45 | 48 => 2,
        3 => 1,
        2 => 0,
        _ => -1,
    }
}

fn c_to(t: f32, unit_f: bool) -> f32 {
    if unit_f { t * 9.0 / 5.0 + 32.0 } else { t }
}

/// Apparent temperature (Australian BoM / Steadman), °C, wind in m/s.
fn apparent(t: f32, rh: f32, ws: f32) -> f32 {
    let e = rh / 100.0 * 6.105 * (17.27 * t / (237.7 + t)).exp();
    t + 0.33 * e - 0.70 * ws - 4.0
}

pub fn parse_met(v: &Value, offset: i64, unit_f: bool) -> Option<Weather> {
    let ts = v.get("properties")?.get("timeseries")?.as_array()?;
    let first = ts.first()?;
    let det = first.get("data")?.get("instant")?.get("details")?;
    let f = |o: &Value, k: &str| o.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    let sym = |e: &Value, k: &str| e.get("data").and_then(|d| d.get(k)).and_then(|n| n.get("summary")).and_then(|s| s.get("symbol_code")).and_then(|s| s.as_str()).map(|s| s.to_string());
    let t = f(det, "air_temperature")?;
    let rh = f(det, "relative_humidity").unwrap_or(50.0);
    let ws = f(det, "wind_speed").unwrap_or(0.0);
    let (code, is_day) = sym(first, "next_1_hours").or_else(|| sym(first, "next_6_hours")).map(|s| met_symbol(&s)).unwrap_or((3, true));
    let mut w = Weather {
        temp: c_to(t, unit_f),
        feels: c_to(apparent(t, rh, ws), unit_f),
        humidity: rh.round() as i32,
        wind: if unit_f { ws * 2.237 } else { ws * 3.6 },
        code,
        is_day,
        offset,
        ..Default::default()
    };
    // Per local day: min/max, worst daytime symbol, rain total and chance.
    struct Acc {
        day: i64,
        max: f32,
        min: f32,
        sev: i32,
        code: i32,
        mm: f32,
        prob: i32,
    }
    let mut days: Vec<Acc> = Vec::new();
    for e in ts {
        let Some(when) = e.get("time").and_then(|x| x.as_str()).and_then(parse_iso_utc) else { continue };
        let local = when + offset;
        let day = local.div_euclid(86400);
        let hour = local.rem_euclid(86400) / 3600;
        let data = e.get("data")?;
        let temp = data.get("instant").and_then(|i| i.get("details")).and_then(|d| f(d, "air_temperature"));
        // Hour by hour for today and tomorrow.
        if let (Some(t), Some(n1)) = (temp, data.get("next_1_hours")) {
            let first_day = w.hours.first().map(|h| h.t.div_euclid(86400)).unwrap_or(day);
            if day - first_day < 2 {
                let det = data.get("instant").and_then(|i| i.get("details"));
                let rh = det.and_then(|d| f(d, "relative_humidity")).unwrap_or(-1.0);
                let ws = det.and_then(|d| f(d, "wind_speed")).unwrap_or(0.0);
                let nd = n1.get("details");
                let code = n1.get("summary").and_then(|s| s.get("symbol_code")).and_then(|s| s.as_str()).map(|s| met_symbol(s).0).unwrap_or(3);
                w.hours.push(Hour {
                    t: local,
                    temp: c_to(t, unit_f),
                    feels: c_to(apparent(t, rh.max(0.0), ws), unit_f),
                    humidity: rh.round() as i32,
                    rain: nd.and_then(|d| f(d, "probability_of_precipitation")).map(|p| p.round() as i32).unwrap_or(-1),
                    mm: nd.and_then(|d| f(d, "precipitation_amount")).unwrap_or(-1.0),
                    code,
                });
            }
        }
        if days.last().map(|d| d.day) != Some(day) {
            if days.len() >= 6 {
                break;
            }
            days.push(Acc { day, max: f32::MIN, min: f32::MAX, sev: -2, code: 0, mm: 0.0, prob: -1 });
        }
        let acc = days.last_mut()?;
        if let Some(t) = temp {
            acc.max = acc.max.max(t);
            acc.min = acc.min.min(t);
        }
        let (period, hours) = if data.get("next_1_hours").is_some() { ("next_1_hours", 1) } else { ("next_6_hours", 6) };
        if let Some(p) = data.get(period) {
            let d = p.get("details");
            // Only count each hour once: 6-hour blocks start at 00/06/12/18.
            if hours == 1 || hour % 6 == 0 {
                acc.mm += d.and_then(|d| f(d, "precipitation_amount")).unwrap_or(0.0);
            }
            if let Some(pp) = d.and_then(|d| f(d, "probability_of_precipitation")) {
                acc.prob = acc.prob.max(pp.round() as i32);
            }
            if (6..=18).contains(&hour) || acc.sev == -2 {
                if let Some(s) = p.get("summary").and_then(|s| s.get("symbol_code")).and_then(|s| s.as_str()) {
                    let (c, _) = met_symbol(s);
                    if severity(c) > acc.sev {
                        acc.sev = severity(c);
                        acc.code = c;
                    }
                }
            }
            if let (Some(mx), Some(mn)) = (d.and_then(|d| f(d, "air_temperature_max")), d.and_then(|d| f(d, "air_temperature_min"))) {
                acc.max = acc.max.max(mx);
                acc.min = acc.min.min(mn);
            }
        }
    }
    for a in days {
        if a.max == f32::MIN {
            continue;
        }
        w.days.push(Day { day: a.day, code: a.code, max: c_to(a.max, unit_f), min: c_to(a.min, unit_f), rain: a.prob, rain_mm: a.mm });
    }
    Some(w)
}

// ---------------------------------------------------------------------------
// wttr.in (World Weather Online data) — JSON format j1
// ---------------------------------------------------------------------------

fn wttr(lat: f64, lon: f64, unit_f: bool) -> Result<Weather, String> {
    let url = format!("https://wttr.in/{lat:.4},{lon:.4}?format=j1");
    let v = net::get_json_with(&url, &[("Accept-Language", "en")])?;
    parse_wttr(&v, unit_f).ok_or_else(|| "Unexpected wttr.in data".to_string())
}

/// World Weather Online condition code → WMO code.
pub fn wwo_to_wmo(c: i32) -> i32 {
    match c {
        113 => 0,
        116 => 2,
        119 | 122 => 3,
        143 | 248 | 260 => 45,
        263 | 266 => 51,
        176 | 353 => 80,
        293 | 296 => 61,
        299 | 302 => 63,
        305 | 308 => 65,
        356 | 359 => 82,
        179 | 182 | 185 | 281 | 284 | 311 | 314 | 317 | 320 | 350 | 362 | 365 | 374 | 377 => 66,
        323 | 326 => 71,
        329 | 332 => 73,
        227 | 230 | 335 | 338 => 75,
        368 | 371 => 85,
        200 | 386 | 389 | 392 | 395 => 95,
        _ => 3,
    }
}

/// "06:12 AM" → minutes after midnight.
fn ampm_minutes(t: &str) -> Option<i64> {
    let t = t.trim();
    let (hm, ap) = t.split_once(' ').unwrap_or((t, ""));
    let (h, m) = hm.split_once(':')?;
    let mut h: i64 = h.trim().parse().ok()?;
    let m: i64 = m.trim().parse().ok()?;
    match ap.to_uppercase().as_str() {
        "PM" if h < 12 => h += 12,
        "AM" if h == 12 => h = 0,
        _ => {}
    }
    Some(h * 60 + m)
}

pub fn parse_wttr(v: &Value, unit_f: bool) -> Option<Weather> {
    // wttr.in sends every number as a string.
    let n = |o: &Value, k: &str| o.get(k).and_then(|x| x.as_str().and_then(|s| s.trim().parse::<f32>().ok()).or_else(|| x.as_f64().map(|f| f as f32)));
    let cur = v.get("current_condition")?.as_array()?.first()?;
    let days = v.get("weather")?.as_array()?;
    let (tk, fk, wk) = if unit_f { ("temp_F", "FeelsLikeF", "windspeedMiles") } else { ("temp_C", "FeelsLikeC", "windspeedKmph") };
    // Day or night from the local observation time vs today's sunrise/sunset.
    let mut is_day = true;
    let obs = cur.get("localObsDateTime").and_then(|x| x.as_str()).unwrap_or("");
    if let (Some(now), Some(astro)) = (obs.split_once(' ').and_then(|(_, t)| ampm_minutes(t)), days.first().and_then(|d| d.get("astronomy")).and_then(|a| a.as_array()).and_then(|a| a.first())) {
        let rise = astro.get("sunrise").and_then(|x| x.as_str()).and_then(ampm_minutes).unwrap_or(6 * 60);
        let set = astro.get("sunset").and_then(|x| x.as_str()).and_then(ampm_minutes).unwrap_or(18 * 60);
        is_day = now >= rise && now < set;
    }
    // Local UTC offset from local observation time vs UTC observation time.
    let mut offset = 0;
    if let (Some(local), Some(utc)) = (obs.split_once(' ').and_then(|(_, t)| ampm_minutes(t)), cur.get("observation_time").and_then(|x| x.as_str()).and_then(ampm_minutes)) {
        let mut d = local - utc;
        if d > 14 * 60 {
            d -= 24 * 60;
        } else if d < -12 * 60 {
            d += 24 * 60;
        }
        offset = (d as f64 / 15.0).round() as i64 * 15 * 60;
    }
    let mut w = Weather {
        temp: n(cur, tk)?,
        feels: n(cur, fk).unwrap_or(0.0),
        humidity: n(cur, "humidity").unwrap_or(0.0) as i32,
        wind: n(cur, wk).unwrap_or(0.0),
        code: wwo_to_wmo(n(cur, "weatherCode").unwrap_or(119.0) as i32),
        is_day,
        offset,
        ..Default::default()
    };
    for d in days {
        let Some(date) = d.get("date").and_then(|x| x.as_str()) else { continue };
        let p: Vec<i64> = date.split('-').filter_map(|s| s.parse().ok()).collect();
        if p.len() != 3 {
            continue;
        }
        let hourly = d.get("hourly").and_then(|h| h.as_array()).cloned().unwrap_or_default();
        // Three-hourly steps for today and tomorrow.
        if w.days.len() < 2 {
            let base = util::days_from_civil(p[0], p[1], p[2]) * 86400;
            let (tkh, fkh) = if unit_f { ("tempF", "FeelsLikeF") } else { ("tempC", "FeelsLikeC") };
            for h in &hourly {
                let Some(tm) = n(h, "time") else { continue };
                let t = n(h, tkh).unwrap_or(w.temp);
                w.hours.push(Hour {
                    t: base + (tm as i64 / 100) * 3600,
                    temp: t,
                    feels: n(h, fkh).unwrap_or(t),
                    humidity: n(h, "humidity").map(|x| x as i32).unwrap_or(-1),
                    rain: n(h, "chanceofrain").map(|x| x as i32).unwrap_or(-1),
                    mm: n(h, "precipMM").unwrap_or(-1.0),
                    code: wwo_to_wmo(n(h, "weatherCode").unwrap_or(119.0) as i32),
                });
            }
        }
        let rain = hourly.iter().filter_map(|h| n(h, "chanceofrain")).fold(-1.0f32, f32::max) as i32;
        let mm: f32 = hourly.iter().filter_map(|h| n(h, "precipMM")).sum::<f32>() * if hourly.len() == 8 { 3.0 } else { 1.0 };
        // Worst daytime condition (slots 09:00–18:00).
        let mut code = 3;
        let mut sev = -2;
        for h in hourly.iter().filter(|h| n(h, "time").map(|t| (900.0..=1800.0).contains(&t)).unwrap_or(false)) {
            let c = wwo_to_wmo(n(h, "weatherCode").unwrap_or(119.0) as i32);
            if severity(c) > sev {
                sev = severity(c);
                code = c;
            }
        }
        let (mx, mn) = if unit_f { ("maxtempF", "mintempF") } else { ("maxtempC", "mintempC") };
        w.days.push(Day {
            day: util::days_from_civil(p[0], p[1], p[2]),
            code,
            max: n(d, mx).unwrap_or(w.temp),
            min: n(d, mn).unwrap_or(w.temp),
            rain,
            rain_mm: mm,
        });
    }
    Some(w)
}

/// "2026-10-27T13:00" (local) → wall-clock seconds.
fn wall_of(t: &str) -> Option<i64> {
    let (d, h) = t.split_once('T')?;
    let p: Vec<i64> = d.split('-').filter_map(|s| s.parse().ok()).collect();
    let q: Vec<i64> = h.split(':').filter_map(|s| s.parse().ok()).collect();
    if p.len() != 3 || q.is_empty() {
        return None;
    }
    Some(util::days_from_civil(p[0], p[1], p[2]) * 86400 + q[0] * 3600 + q.get(1).copied().unwrap_or(0) * 60)
}

pub fn parse_forecast(v: &Value) -> Option<Weather> {
    let cur = v.get("current")?;
    let f = |o: &Value, k: &str| o.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
    let mut w = Weather {
        temp: f(cur, "temperature_2m"),
        feels: cur.get("apparent_temperature").and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or_else(|| f(cur, "temperature_2m")),
        humidity: f(cur, "relative_humidity_2m") as i32,
        wind: f(cur, "wind_speed_10m"),
        code: f(cur, "weather_code") as i32,
        is_day: f(cur, "is_day") >= 1.0,
        offset: v.get("utc_offset_seconds").and_then(|x| x.as_i64()).unwrap_or(0),
        abbr: v.get("timezone_abbreviation").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        zone: v.get("timezone").and_then(|x| x.as_str()).filter(|z| z.contains('/')).unwrap_or("").to_string(),
        ..Default::default()
    };
    if let Some(h) = v.get("hourly") {
        let arr = |k: &str| h.get(k).and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let (t, tt, ft, rh, pp, mm, wc) = (arr("time"), arr("temperature_2m"), arr("apparent_temperature"), arr("relative_humidity_2m"), arr("precipitation_probability"), arr("precipitation"), arr("weather_code"));
        let num = |a: &Vec<Value>, i: usize| a.get(i).and_then(|x| x.as_f64());
        for i in 0..t.len().min(48) {
            let Some(ts) = t[i].as_str().and_then(wall_of) else { continue };
            w.hours.push(Hour {
                t: ts,
                temp: num(&tt, i).unwrap_or(0.0) as f32,
                feels: num(&ft, i).or(num(&tt, i)).unwrap_or(0.0) as f32,
                humidity: num(&rh, i).map(|x| x as i32).unwrap_or(-1),
                rain: num(&pp, i).map(|x| x as i32).unwrap_or(-1),
                mm: num(&mm, i).map(|x| x as f32).unwrap_or(-1.0),
                code: num(&wc, i).unwrap_or(0.0) as i32,
            });
        }
    }
    if let Some(d) = v.get("daily") {
        let arr = |k: &str| d.get(k).and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let (t, c, mx, mn, r, mm) = (arr("time"), arr("weather_code"), arr("temperature_2m_max"), arr("temperature_2m_min"), arr("precipitation_probability_max"), arr("precipitation_sum"));
        for i in 0..t.len() {
            let Some(date) = t[i].as_str() else { continue };
            let p: Vec<i64> = date.split('-').filter_map(|s| s.parse().ok()).collect();
            if p.len() != 3 {
                continue;
            }
            w.days.push(Day {
                day: util::days_from_civil(p[0], p[1], p[2]),
                code: c.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as i32,
                max: mx.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                min: mn.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                rain: r.get(i).and_then(|x| x.as_f64()).map(|x| x as i32).unwrap_or(-1),
                rain_mm: mm.get(i).and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(-1.0),
            });
        }
    }
    Some(w)
}

/// WMO weather code → short description.
pub fn describe(code: i32) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Cloudy",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 => "Light rain",
        63 => "Rain",
        65 => "Heavy rain",
        66 | 67 => "Freezing rain",
        71 | 73 | 75 | 77 => "Snow",
        80 | 81 => "Showers",
        82 => "Heavy showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm, hail",
        _ => "—",
    }
}
