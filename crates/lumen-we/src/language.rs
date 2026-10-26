//! The `language` general property WE gives web wallpapers: one of 36 codes
//! (`en-us`, `de-de`, `zh-chs`, …). WE picks it from the language chosen in
//! its settings, or from Steam's language when that is `default`
//! (`webwallpaper64.exe`, see docs/re-notes/web-adjustments.md).

/// (WE code, Steam language name), in WE's order.
const LANGUAGES: [(&str, &str); 36] = [
    ("en-us", "english"),
    ("de-de", "german"),
    ("pt-br", "brazilian"),
    ("bg-bg", "bulgarian"),
    ("cs-cz", "czech"),
    ("da-dk", "danish"),
    ("nl-nl", "dutch"),
    ("fi-fi", "finnish"),
    ("fr-fr", "french"),
    ("el-gr", "greek"),
    ("hu-hu", "hungarian"),
    ("it-it", "italian"),
    ("ja-jp", "japanese"),
    ("ko-kr", "koreana"),
    ("nb-no", "norwegian"),
    ("pl-pl", "polish"),
    ("pt-pt", "portuguese"),
    ("ro-ro", "romanian"),
    ("ru-ru", "russian"),
    ("zh-chs", "schinese"),
    ("es-es", "spanish"),
    ("sv-se", "swedish"),
    ("zh-cht", "tchinese"),
    ("th-th", "thai"),
    ("tr-tr", "turkish"),
    ("ar-sa", "arabic"),
    ("uk-ua", "ukrainian"),
    ("lt-lt", "lithuanian"),
    ("id-id", "indonesian"),
    ("vi-vn", "vietnamese"),
    ("he-il", "hebrew"),
    ("sk-sk", "slovak"),
    ("sl-si", "slovenian"),
    ("eu-es", "basque"),
    ("fa-ir", "farsi"),
    ("be-by", "belarusian"),
];

/// WE's code for a Steam language name (`latam` counts as Spanish).
pub fn from_steam(name: &str) -> Option<&'static str> {
    let name = name.trim().to_ascii_lowercase();
    if name == "latam" {
        return Some("es-es");
    }
    LANGUAGES
        .iter()
        .find(|(_, steam)| *steam == name)
        .map(|(code, _)| *code)
}

/// WE's code for a language setting: a WE code or a Steam name.
pub fn from_setting(value: &str) -> Option<&'static str> {
    let value = value.trim().to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|(code, _)| *code == value)
        .map(|(code, _)| *code)
        .or_else(|| from_steam(&value))
}

/// The closest WE code for an OS language tag (`en-GB`, `zh-Hant-TW`,
/// `pt_BR.UTF-8`); English when none is close.
pub fn from_locale(tag: &str) -> &'static str {
    let tag = tag
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .replace('_', "-")
        .to_ascii_lowercase();
    let mut parts = tag.split('-');
    let lang = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    let has = |s: &str| rest.contains(&s);
    match lang {
        "zh" if has("hant") || has("tw") || has("hk") || has("mo") => "zh-cht",
        "zh" => "zh-chs",
        "pt" if has("br") => "pt-br",
        "pt" => "pt-pt",
        "no" | "nn" | "nb" => "nb-no",
        _ => LANGUAGES
            .iter()
            .find(|(code, _)| code.split('-').next() == Some(lang))
            .map_or("en-us", |(code, _)| *code),
    }
}

/// The code WE would give pages for its settings (`general.user` of
/// `config.json`): the chosen `language`, or Steam's when it is `default`.
pub fn from_we_settings(
    language: Option<&str>,
    steam_language: Option<&str>,
) -> Option<&'static str> {
    match language.map(str::trim) {
        Some(l) if !l.is_empty() && !l.eq_ignore_ascii_case("default") => from_setting(l),
        _ => steam_language.and_then(from_steam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_names_and_settings() {
        assert_eq!(from_steam("english"), Some("en-us"));
        assert_eq!(from_steam("koreana"), Some("ko-kr"));
        assert_eq!(from_steam("latam"), Some("es-es"));
        assert_eq!(from_steam("klingon"), None);
        assert_eq!(from_setting("DE-DE"), Some("de-de"));
        assert_eq!(
            from_we_settings(Some("default"), Some("schinese")),
            Some("zh-chs")
        );
        assert_eq!(
            from_we_settings(Some("french"), Some("english")),
            Some("fr-fr")
        );
        assert_eq!(from_we_settings(None, None), None);
    }

    #[test]
    fn os_locales() {
        assert_eq!(from_locale("en-GB"), "en-us");
        assert_eq!(from_locale("de_AT.UTF-8"), "de-de");
        assert_eq!(from_locale("zh-Hant-TW"), "zh-cht");
        assert_eq!(from_locale("zh-CN"), "zh-chs");
        assert_eq!(from_locale("pt-BR"), "pt-br");
        assert_eq!(from_locale("pt"), "pt-pt");
        assert_eq!(from_locale("nn-NO"), "nb-no");
        assert_eq!(from_locale("es-MX"), "es-es");
        assert_eq!(from_locale("C"), "en-us");
        assert_eq!(from_locale(""), "en-us");
    }
}
