//! The Vietnamese lunar calendar (Âm lịch), worked out on the PC: new moons
//! and solar terms from Hồ Ngọc Đức's well-known algorithm, for UTC+7.
//! Days are counted as in `util` (days since 1970-01-01).

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;

const TZ: f64 = 7.0;
/// Julian day number of 1970-01-01.
const JD_EPOCH: i64 = 2_440_588;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LDate {
    pub day: i64,
    pub month: i64,
    pub year: i64,
    pub leap: bool,
}

pub const CAN: [&str; 10] = ["Giáp", "Ất", "Bính", "Đinh", "Mậu", "Kỷ", "Canh", "Tân", "Nhâm", "Quý"];
pub const CHI: [&str; 12] = ["Tý", "Sửu", "Dần", "Mão", "Thìn", "Tỵ", "Ngọ", "Mùi", "Thân", "Dậu", "Tuất", "Hợi"];

fn jd_from_date(dd: i64, mm: i64, yy: i64) -> i64 {
    let a = (14 - mm) / 12;
    let y = yy + 4800 - a;
    let m = mm + 12 * a - 3;
    let jd = dd + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045;
    if jd < 2_299_161 {
        dd + (153 * m + 2) / 5 + 365 * y + y / 4 - 32083
    } else {
        jd
    }
}

/// The k-th new moon after 1900-01-01 (Julian day, fractional, UT).
fn new_moon(k: i64) -> f64 {
    let k = k as f64;
    let t = k / 1236.85;
    let (t2, t3) = (t * t, t * t * t);
    let dr = PI / 180.0;
    let mut jd1 = 2415020.75933 + 29.53058868 * k + 0.0001178 * t2 - 0.000000155 * t3;
    jd1 += 0.00033 * ((166.56 + 132.87 * t - 0.009173 * t2) * dr).sin();
    let m = 359.2242 + 29.10535608 * k - 0.0000333 * t2 - 0.00000347 * t3;
    let mpr = 306.0253 + 385.81691806 * k + 0.0107306 * t2 + 0.00001236 * t3;
    let f = 21.2964 + 390.67050646 * k - 0.0016528 * t2 - 0.00000239 * t3;
    let mut c1 = (0.1734 - 0.000393 * t) * (m * dr).sin() + 0.0021 * (2.0 * dr * m).sin();
    c1 = c1 - 0.4068 * (mpr * dr).sin() + 0.0161 * (dr * 2.0 * mpr).sin();
    c1 -= 0.0004 * (dr * 3.0 * mpr).sin();
    c1 = c1 + 0.0104 * (dr * 2.0 * f).sin() - 0.0051 * (dr * (m + mpr)).sin();
    c1 = c1 - 0.0074 * (dr * (m - mpr)).sin() + 0.0004 * (dr * (2.0 * f + m)).sin();
    c1 = c1 - 0.0004 * (dr * (2.0 * f - m)).sin() - 0.0006 * (dr * (2.0 * f + mpr)).sin();
    c1 = c1 + 0.0010 * (dr * (2.0 * f - mpr)).sin() + 0.0005 * (dr * (2.0 * mpr + m)).sin();
    let deltat = if t < -11.0 {
        0.001 + 0.000839 * t + 0.0002261 * t2 - 0.00000845 * t3 - 0.000000081 * t * t3
    } else {
        -0.000278 + 0.000265 * t + 0.000262 * t2
    };
    jd1 + c1 - deltat
}

fn new_moon_day(k: i64) -> i64 {
    (new_moon(k) + 0.5 + TZ / 24.0).floor() as i64
}

/// The sun's longitude (radians, 0..2π) at Julian day `jdn`.
fn sun_longitude(jdn: f64) -> f64 {
    let t = (jdn - 2451545.0) / 36525.0;
    let t2 = t * t;
    let dr = PI / 180.0;
    let m = 357.52910 + 35999.05030 * t - 0.0001559 * t2 - 0.00000048 * t * t2;
    let l0 = 280.46645 + 36000.76983 * t + 0.0003032 * t2;
    let mut dl = (1.914600 - 0.004817 * t - 0.000014 * t2) * (dr * m).sin();
    dl += (0.019993 - 0.000101 * t) * (dr * 2.0 * m).sin() + 0.000290 * (dr * 3.0 * m).sin();
    let mut l = (l0 + dl) * dr;
    l -= PI * 2.0 * (l / (PI * 2.0)).floor();
    l
}

/// Which of the 12 major solar terms (0..11) the day starts in.
fn sun_sector(day_number: i64) -> i64 {
    (sun_longitude(day_number as f64 - 0.5 - TZ / 24.0) / PI * 6.0).floor() as i64
}

/// Day of the new moon that starts lunar month 11 of `yy`.
fn month11(yy: i64) -> i64 {
    let off = jd_from_date(31, 12, yy) - 2415021;
    let k = (off as f64 / 29.530588853).floor() as i64;
    let nm = new_moon_day(k);
    if sun_sector(nm) >= 9 {
        new_moon_day(k - 1)
    } else {
        nm
    }
}

/// Months after month 11 until the leap month (in a 13-month year).
fn leap_offset(a11: i64) -> i64 {
    let k = ((a11 as f64 - 2415021.076998695) / 29.530588853 + 0.5).floor() as i64;
    let mut i = 1;
    let mut arc = sun_sector(new_moon_day(k + i));
    loop {
        let last = arc;
        i += 1;
        arc = sun_sector(new_moon_day(k + i));
        if !(arc != last && i < 14) {
            break;
        }
    }
    i - 1
}

fn compute(day: i64) -> LDate {
    let jd = day + JD_EPOCH;
    let (yy, _, _) = crate::util::civil_from_days(day);
    let k = ((jd as f64 - 2415021.076998695) / 29.530588853).floor() as i64;
    let mut month_start = new_moon_day(k + 1);
    if month_start > jd {
        month_start = new_moon_day(k);
    }
    let mut a11 = month11(yy);
    let mut b11 = a11;
    let mut year;
    if a11 >= month_start {
        year = yy;
        a11 = month11(yy - 1);
    } else {
        year = yy + 1;
        b11 = month11(yy + 1);
    }
    let lday = jd - month_start + 1;
    let diff = ((month_start - a11) as f64 / 29.0).floor() as i64;
    let mut leap = false;
    let mut month = diff + 11;
    if b11 - a11 > 365 {
        let lm = leap_offset(a11);
        if diff >= lm {
            month = diff + 10;
            if diff == lm {
                leap = true;
            }
        }
    }
    if month > 12 {
        month -= 12;
    }
    if month >= 11 && diff < 4 {
        year -= 1;
    }
    LDate { day: lday, month, year, leap }
}

thread_local! {
    static MEMO: RefCell<HashMap<i64, LDate>> = RefCell::new(HashMap::new());
}

/// The lunar date of a day (remembered: the calendar asks often).
pub fn of(day: i64) -> LDate {
    MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 4000 {
            m.clear();
        }
        *m.entry(day).or_insert_with(|| compute(day))
    })
}

/// The lunar month that day `day` is in: (first day, number of days).
pub fn month_of(day: i64) -> (i64, i64) {
    let jd = day + JD_EPOCH;
    let k = ((jd as f64 - 2415021.076998695) / 29.530588853).floor() as i64;
    let mut kk = k + 1;
    if new_moon_day(kk) > jd {
        kk = k;
    }
    if new_moon_day(kk) > jd {
        kk -= 1;
    }
    let start = new_moon_day(kk);
    let next = new_moon_day(kk + 1);
    (start - JD_EPOCH, next - start)
}

/// The lunar month `n` months after the one holding `day` (n may be negative).
pub fn month_shift(day: i64, n: i64) -> (i64, i64) {
    let (start, _) = month_of(day);
    // Step by about a month, landing mid-month to stay clear of the edges.
    month_of(start + 15 + (n as f64 * 29.530588853).round() as i64)
}

pub fn year_name(y: i64) -> String {
    format!("{} {}", CAN[((y + 6) % 10) as usize], CHI[((y + 8) % 12) as usize])
}

pub fn day_name(day: i64) -> String {
    let jd = day + JD_EPOCH;
    format!("{} {}", CAN[((jd + 9) % 10) as usize], CHI[((jd + 1) % 12) as usize])
}

#[allow(dead_code)]
pub fn month_name(m: i64, y: i64) -> String {
    format!("{} {}", CAN[((y * 12 + m + 3) % 10) as usize], CHI[((m + 1) % 12) as usize])
}

/// The solar terms (Tiết khí), from the March equinox (0°) on.
pub const TERMS: [&str; 24] = [
    "Xuân phân", "Thanh minh", "Cốc vũ", "Lập hạ", "Tiểu mãn", "Mang chủng", "Hạ chí", "Tiểu thử", "Đại thử", "Lập thu", "Xử thử", "Bạch lộ",
    "Thu phân", "Hàn lộ", "Sương giáng", "Lập đông", "Tiểu tuyết", "Đại tuyết", "Đông chí", "Tiểu hàn", "Đại hàn", "Lập xuân", "Vũ thủy", "Kinh trập",
];

/// The twelve day / hour spirits; hoàng đạo (lucky) ones are marked true.
pub const GODS: [(&str, bool); 12] = [
    ("Thanh Long", true),
    ("Minh Đường", true),
    ("Thiên Hình", false),
    ("Chu Tước", false),
    ("Kim Quỹ", true),
    ("Kim Đường", true),
    ("Bạch Hổ", false),
    ("Ngọc Đường", true),
    ("Thiên Lao", false),
    ("Huyền Vũ", false),
    ("Tư Mệnh", true),
    ("Câu Trận", false),
];

/// The twelve "Trực" of a day.
pub const TRUC: [&str; 12] = ["Kiến", "Trừ", "Mãn", "Bình", "Định", "Chấp", "Phá", "Nguy", "Thành", "Thu", "Khai", "Bế"];

/// Western star signs (Cung hoàng đạo): (Vietnamese, English, symbol).
pub const SIGNS: [(&str, &str, &str); 12] = [
    ("Ma Kết", "Capricorn", "♑"),
    ("Bảo Bình", "Aquarius", "♒"),
    ("Song Ngư", "Pisces", "♓"),
    ("Bạch Dương", "Aries", "♈"),
    ("Kim Ngưu", "Taurus", "♉"),
    ("Song Tử", "Gemini", "♊"),
    ("Cự Giải", "Cancer", "♋"),
    ("Sư Tử", "Leo", "♌"),
    ("Xử Nữ", "Virgo", "♍"),
    ("Thiên Bình", "Libra", "♎"),
    ("Bọ Cạp", "Scorpio", "♏"),
    ("Nhân Mã", "Sagittarius", "♐"),
];

/// Everything the calendar says about a day.
#[derive(Clone, Debug)]
pub struct DayInfo {
    /// The day's spirit (index into GODS): hoàng đạo when GODS[god].1.
    pub god: usize,
    /// The 12 two-hour periods (index = chi), hoàng đạo or not.
    pub hours: [bool; 12],
    pub term: usize,
    pub truc: usize,
    /// Star sign (index into SIGNS).
    pub sign: usize,
    /// The chi this day clashes with (xung).
    pub clash: usize,
    /// "Tam nương" / "Nguyệt kỵ" days (traditionally avoided).
    pub warn: Option<&'static str>,
}

pub fn day_chi(day: i64) -> usize {
    ((day + JD_EPOCH + 1).rem_euclid(12)) as usize
}

/// The solar term the day falls in.
pub fn term_of(day: i64) -> usize {
    let l = sun_longitude((day + JD_EPOCH) as f64 + 0.5 - TZ / 24.0);
    ((l / (2.0 * PI) * 24.0).floor() as i64).rem_euclid(24) as usize
}

pub fn info(day: i64) -> DayInfo {
    let dc = day_chi(day);
    let term = term_of(day);
    // The month by solar terms (Lập xuân starts Dần).
    let month_chi = (2 + ((term + 3) % 24) / 2) % 12;
    let day_start = ((month_chi + 12 - 2) % 6) * 2;
    let god = (dc + 12 - day_start) % 12;
    let hour_start = ((dc % 6) * 2 + 8) % 12;
    let mut hours = [false; 12];
    for (h, x) in hours.iter_mut().enumerate() {
        *x = GODS[(h + 12 - hour_start) % 12].1;
    }
    let (_, m, d) = crate::util::civil_from_days(day);
    // Sign boundaries (day each sign starts), Capricorn from 22 Dec.
    const START: [(u32, u32); 12] = [(12, 22), (1, 20), (2, 19), (3, 21), (4, 20), (5, 21), (6, 22), (7, 23), (8, 23), (9, 23), (10, 24), (11, 22)];
    let md = (m as u32) * 100 + d as u32;
    let mut sign = 0;
    for (i, (sm, sd)) in START.iter().enumerate().skip(1) {
        if md >= sm * 100 + sd {
            sign = i;
        }
    }
    if md >= 1222 {
        sign = 0;
    }
    let l = of(day);
    let warn = if [3, 7, 13, 18, 22, 27].contains(&l.day) {
        Some("Tam nương")
    } else if [5, 14, 23].contains(&l.day) {
        Some("Nguyệt kỵ")
    } else {
        None
    };
    DayInfo { god, hours, term, truc: (dc + 12 - month_chi) % 12, sign, clash: (dc + 6) % 12, warn }
}

/// "23–1h" for a two-hour period (by chi).
pub fn hour_span(chi: usize) -> String {
    let a = (23 + 2 * chi) % 24;
    format!("{a}–{}h", (a + 2) % 24)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_dates() {
        // Tết 2024 = 10 Feb 2024; Tết 2025 = 29 Jan 2025; Tết 2026 = 17 Feb 2026.
        let d = |y, m, dd| crate::util::days_from_civil(y, m, dd);
        assert_eq!(of(d(2024, 2, 10)), LDate { day: 1, month: 1, year: 2024, leap: false });
        assert_eq!(of(d(2025, 1, 29)), LDate { day: 1, month: 1, year: 2025, leap: false });
        assert_eq!(of(d(2026, 2, 17)), LDate { day: 1, month: 1, year: 2026, leap: false });
        // Hoàng đạo hours of a Tý / Ngọ day: Tý, Sửu, Mão, Ngọ, Thân, Dậu.
        let mut k = d(2024, 2, 10);
        while day_chi(k) != 0 {
            k += 1;
        }
        let h = info(k).hours;
        assert_eq!((0..12).filter(|&i| h[i]).collect::<Vec<_>>(), vec![0, 1, 3, 6, 8, 9]);
        // 27 Oct 2026 is a Scorpio; 1 Jan a Capricorn; 21 Mar an Aries.
        assert_eq!(SIGNS[info(d(2026, 10, 27)).sign].1, "Scorpio");
        assert_eq!(SIGNS[info(d(2026, 1, 1)).sign].1, "Capricorn");
        assert_eq!(SIGNS[info(d(2026, 3, 21)).sign].1, "Aries");
        // Đông chí ~ 21-22 Dec, Lập xuân ~ 4 Feb.
        assert_eq!(TERMS[term_of(d(2026, 12, 23))], "Đông chí");
        assert_eq!(TERMS[term_of(d(2026, 2, 5))], "Lập xuân");
        // 2025 has a leap 6th month starting 25 Jul 2025.
        assert_eq!(of(d(2025, 7, 25)), LDate { day: 1, month: 6, year: 2025, leap: true });
        assert_eq!(year_name(2026), "Bính Ngọ");
    }
}
