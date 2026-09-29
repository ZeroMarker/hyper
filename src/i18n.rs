/// Language for Hyper-owned interface text. Model replies and tool output are
/// displayed as received and are not translated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    Chinese,
}

impl Language {
    pub fn parse(value: &str) -> Self {
        let tag = value.trim().split(['-', '_', '.']).next().unwrap_or("");
        if tag.eq_ignore_ascii_case("zh") {
            Self::Chinese
        } else {
            Self::English
        }
    }

    pub fn text(self, english: &'static str, chinese: &'static str) -> &'static str {
        match self {
            Self::English => english,
            Self::Chinese => chinese,
        }
    }
}

pub fn language() -> Language {
    Language::parse(&std::env::var("HYPER_LANG").unwrap_or_default())
}

pub fn text(english: &'static str, chinese: &'static str) -> &'static str {
    language().text(english, chinese)
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn english_is_the_default_and_chinese_needs_an_explicit_tag() {
        for tag in ["", "en", "en-US", "fr", "invalid"] {
            assert_eq!(Language::parse(tag), Language::English);
        }
        for tag in ["zh", "zh-CN", "ZH_TW", "zh_CN.UTF-8"] {
            assert_eq!(Language::parse(tag), Language::Chinese);
        }
    }
}
