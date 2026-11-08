//! The island's languages. Strings are written in English in the code and
//! looked up here; anything missing simply stays English. Names that read
//! the same everywhere (Dynamic Island, Windows, Wi-Fi, IP…) stay as they are.

use crate::config;

/// Settings → General → Language, in this order.
pub const LANGS: [&str; 7] = ["English", "Tiếng Việt", "中文", "한국어", "日本語", "Français", "Español"];

pub fn lang() -> usize {
    (config::cur().i("lang").max(0) as usize).min(LANGS.len() - 1)
}

pub fn vi() -> bool {
    lang() == 1
}

/// The Windows locale for dates in the island's language.
pub fn locale() -> &'static str {
    ["en-US", "vi-VN", "zh-CN", "ko-KR", "ja-JP", "fr-FR", "es-ES"][lang()]
}

/// The current language's version of an English UI string.
pub fn tr(s: &str) -> &str {
    let table: &[(&str, &str)] = match lang() {
        1 => TABLE_VI,
        2 => TABLE_ZH,
        3 => TABLE_KO,
        4 => TABLE_JA,
        5 => TABLE_FR,
        6 => TABLE_ES,
        _ => return s,
    };
    match table.binary_search_by(|(en, _)| (*en).cmp(s)) {
        Ok(i) => table[i].1,
        Err(_) => s,
    }
}

include!("i18n_table.rs");
