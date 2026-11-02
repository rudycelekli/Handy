//! Simplified / Traditional script handling for Chinese transcription output.
//!
//! Recognition language (Mandarin vs Cantonese) and output script are separate
//! choices: the language picks what the model listens for, and the
//! [`ChineseScript`] setting picks how the resulting text is written. Conversion
//! is keyed off the language the output is known to be in, so mixed-language
//! users on auto-detect only get Chinese output rewritten.

use crate::settings::ChineseScript;
use ferrous_opencc::{config::BuiltinConfig, OpenCC};
use log::error;
use std::sync::OnceLock;

/// Spoken Chinese variety; selects the Simplified tables, which differ for Cantonese.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChineseVariety {
    Mandarin,
    Cantonese,
}

impl ChineseVariety {
    /// Maps an output language code (`zh`, `zh-CN`, `yue`, …) to a Chinese
    /// variety. Any other language returns `None` and is never converted.
    pub fn from_language(language: &str) -> Option<Self> {
        let base = language.split(&['-', '_'][..]).next()?.to_ascii_lowercase();
        match base.as_str() {
            "zh" => Some(Self::Mandarin),
            "yue" => Some(Self::Cantonese),
            _ => None,
        }
    }
}

/// The variety to convert `text` as, given the language the output is known
/// to be in.
///
/// A Chinese language picks its variety directly. Mixed Mandarin/English
/// speech is often detected as English as a whole (#2239), so text that is
/// known to be in another language still counts as Mandarin when it contains
/// Han characters. Japanese and Korean are excluded, and so is any text with
/// kana, so kanji are never rewritten.
pub fn variety_for_output(language: Option<&str>, text: &str) -> Option<ChineseVariety> {
    let language = language?;
    if let Some(variety) = ChineseVariety::from_language(language) {
        return Some(variety);
    }
    let base = language.split(&['-', '_'][..]).next()?.to_ascii_lowercase();
    if matches!(base.as_str(), "ja" | "ko") {
        return None;
    }
    let has_han = text.chars().any(is_han);
    let has_kana = text.chars().any(is_kana);
    (has_han && !has_kana).then_some(ChineseVariety::Mandarin)
}

fn is_han(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{20000}'..='\u{3134F}')
}

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{FF66}'..='\u{FF9F}')
}

/// The script a locale writes Chinese in, or `None` for non-Chinese locales.
///
/// `zh` is Simplified unless tagged Hant or regioned to Taiwan, Hong Kong or
/// Macau; Cantonese (`yue`) is Traditional unless tagged Hans. Traditional
/// takes Hong Kong's forms for Hong Kong, Macau and Cantonese, else Taiwan's.
pub fn chinese_script_for_locale(locale: &str) -> Option<ChineseScript> {
    let normalized = locale.to_lowercase().replace('_', "-");
    let subtags: Vec<_> = normalized.split('-').collect();
    let has = |subtag: &str| subtags.contains(&subtag);
    let hong_kong = has("hk") || has("mo");

    let script = match subtags.first().copied() {
        Some("zh") if has("hans") => ChineseScript::Simplified,
        Some("zh") if hong_kong => ChineseScript::TraditionalHk,
        Some("zh") if has("hant") || has("tw") => ChineseScript::TraditionalTw,
        Some("zh") => ChineseScript::Simplified,
        Some("yue") if has("hans") => ChineseScript::Simplified,
        Some("yue") if has("tw") => ChineseScript::TraditionalTw,
        Some("yue") => ChineseScript::TraditionalHk,
        _ => return None,
    };
    Some(script)
}

/// The OpenCC conversion for a variety and script. Traditional converts directly to
/// the region's forms; going via Simplified would mangle correct text (颱風後 → 台風后).
fn conversion(variety: ChineseVariety, script: ChineseScript) -> Option<BuiltinConfig> {
    match (variety, script) {
        (_, ChineseScript::AsTranscribed) => None,
        (ChineseVariety::Mandarin, ChineseScript::Simplified) => Some(BuiltinConfig::Tw2sp),
        (ChineseVariety::Cantonese, ChineseScript::Simplified) => Some(BuiltinConfig::Hk2s),
        (_, ChineseScript::TraditionalTw) => Some(BuiltinConfig::S2tw),
        (_, ChineseScript::TraditionalHk) => Some(BuiltinConfig::S2hk),
    }
}

/// The OpenCC converter for a variety/script pair, built on first use and
/// reused after. Building one parses its dictionaries, which is too slow to
/// repeat on every live-preview update. A failed build is cached as `None`.
fn converter(variety: ChineseVariety, script: ChineseScript) -> Option<&'static OpenCC> {
    static TW2SP: OnceLock<Option<OpenCC>> = OnceLock::new();
    static HK2S: OnceLock<Option<OpenCC>> = OnceLock::new();
    static S2TW: OnceLock<Option<OpenCC>> = OnceLock::new();
    static S2HK: OnceLock<Option<OpenCC>> = OnceLock::new();

    let config = conversion(variety, script)?;
    let cell = match config {
        BuiltinConfig::Tw2sp => &TW2SP,
        BuiltinConfig::Hk2s => &HK2S,
        BuiltinConfig::S2tw => &S2TW,
        _ => &S2HK,
    };

    cell.get_or_init(|| match OpenCC::from_config(config) {
        Ok(converter) => Some(converter),
        Err(e) => {
            error!(
                "Failed to initialize OpenCC converter: {}. Keeping the original script.",
                e
            );
            None
        }
    })
    .as_ref()
}

/// Rewrites `text` into `script`. Fails open: if OpenCC can't be initialized
/// the text is returned unchanged.
pub fn convert_chinese_script(
    text: &str,
    variety: ChineseVariety,
    script: ChineseScript,
) -> String {
    match converter(variety, script) {
        Some(converter) => converter.convert(text),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cantonese_uses_hong_kong_tables() {
        // Hong Kong standard writes 裏 where Taiwan writes 裡.
        assert_eq!(
            convert_chinese_script(
                "里面",
                ChineseVariety::Cantonese,
                ChineseScript::TraditionalHk
            ),
            "裏面"
        );
        assert_eq!(
            convert_chinese_script("裏面", ChineseVariety::Cantonese, ChineseScript::Simplified),
            "里面"
        );
    }

    #[test]
    fn mixed_chinese_detected_as_english_is_converted() {
        // #2239: Whisper reports `en` for mixed Mandarin/English speech.
        let text = "我跟你说，Oh my god, I forgot to charge my laptop.";
        let variety = variety_for_output(Some("en"), text);
        assert_eq!(variety, Some(ChineseVariety::Mandarin));
        assert_eq!(
            convert_chinese_script(text, variety.unwrap(), ChineseScript::TraditionalTw),
            "我跟你說，Oh my god, I forgot to charge my laptop."
        );
    }

    #[test]
    fn variety_for_output_leaves_other_text_alone() {
        assert_eq!(variety_for_output(Some("en"), "Hello there."), None);
        assert_eq!(variety_for_output(None, "我跟你说"), None);
        // Japanese: kanji must never be rewritten.
        assert_eq!(variety_for_output(Some("ja"), "会议"), None);
        assert_eq!(variety_for_output(Some("en"), "会議に行きます"), None);
        assert_eq!(variety_for_output(Some("ko"), "漢字"), None);
        assert_eq!(
            variety_for_output(Some("yue"), "hello"),
            Some(ChineseVariety::Cantonese)
        );
        assert_eq!(
            variety_for_output(Some("zh-TW"), "hello"),
            Some(ChineseVariety::Mandarin)
        );
    }
}
