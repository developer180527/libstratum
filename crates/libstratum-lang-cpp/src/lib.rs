//! C and C++ language support.

use libstratum_core::ir::SourceLanguage;
use libstratum_core::spi::LanguageSupport;

#[derive(Debug, Clone, Copy, Default)]
pub struct CCpp;

impl LanguageSupport for CCpp {
    fn id(&self) -> &'static str {
        "c-cpp"
    }

    fn claims(&self, language: SourceLanguage) -> bool {
        matches!(language, SourceLanguage::C | SourceLanguage::Cpp)
    }

    fn normalize_type_name(&self, name: &str) -> String {
        normalize(name)
    }
}

/// Canonical spellings for fundamental types. Each entry is a set of words in any order
/// (GCC writes `long unsigned int`, Clang `unsigned long`, MSVC `unsigned __int64`).
const CANONICAL: &[(&[&str], &str)] = &[
    (&["unsigned", "long", "long", "int"], "unsigned long long"),
    (&["unsigned", "long", "long"], "unsigned long long"),
    (&["signed", "long", "long", "int"], "long long"),
    (&["long", "long", "int"], "long long"),
    (&["signed", "long", "long"], "long long"),
    (&["unsigned", "__int64"], "unsigned long long"),
    (&["__int64"], "long long"),
    (&["unsigned", "long", "int"], "unsigned long"),
    (&["signed", "long", "int"], "long"),
    (&["long", "int"], "long"),
    (&["signed", "long"], "long"),
    (&["unsigned", "short", "int"], "unsigned short"),
    (&["signed", "short", "int"], "short"),
    (&["short", "int"], "short"),
    (&["signed", "short"], "short"),
    (&["unsigned", "__int32"], "unsigned int"),
    (&["__int32"], "int"),
    (&["unsigned", "__int16"], "unsigned short"),
    (&["__int16"], "short"),
    (&["unsigned"], "unsigned int"),
    (&["signed", "int"], "int"),
    (&["signed"], "int"),
];

/// Normalizes a qualified C/C++ type name so spellings from Clang, GCC and MSVC compare equal:
/// whitespace around punctuation is removed (`> >` → `>>`, `, ` → `,`) and runs of fundamental
/// type keywords are canonicalized (`short int` → `short`, `long unsigned int` → `unsigned long`).
pub fn normalize(name: &str) -> String {
    // Tokenize into identifier-ish words and single punctuation characters.
    let mut tokens: Vec<String> = Vec::new();
    let mut word = String::new();
    for ch in name.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            word.push(ch);
        } else {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            if !ch.is_whitespace() {
                tokens.push(ch.to_string());
            }
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }

    // Canonicalize maximal runs of type keywords.
    const KEYWORDS: &[&str] = &["unsigned", "signed", "short", "long", "int", "__int16", "__int32", "__int64"];
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if KEYWORDS.contains(&tokens[i].as_str()) {
            let start = i;
            while i < tokens.len() && KEYWORDS.contains(&tokens[i].as_str()) {
                i += 1;
            }
            let mut run: Vec<&str> = tokens[start..i].iter().map(String::as_str).collect();
            run.sort_unstable();
            let canonical = CANONICAL.iter().find(|(words, _)| {
                let mut words = words.to_vec();
                words.sort_unstable();
                words == run
            });
            match canonical {
                Some((_, spelling)) => out.push((*spelling).to_owned()),
                None => out.push(tokens[start..i].join(" ")),
            }
        } else {
            out.push(tokens[i].clone());
            i += 1;
        }
    }

    // Re-join: a space only between two word tokens.
    let mut result = String::new();
    for token in &out {
        let is_word = token.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let prev_word = result.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_');
        if is_word && prev_word {
            result.push(' ');
        }
        result.push_str(token);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_only_c_family() {
        assert!(CCpp.claims(SourceLanguage::C));
        assert!(CCpp.claims(SourceLanguage::Cpp));
        assert!(!CCpp.claims(SourceLanguage::Rust));
    }

    #[test]
    fn msvc_and_clang_template_spellings_match() {
        assert_eq!(
            normalize("std::vector<int,std::allocator<int> >"),
            normalize("std::vector<int, std::allocator<int>>")
        );
    }

    #[test]
    fn gcc_integer_spellings_match_clang() {
        assert_eq!(normalize("SmallArray<short int, 3>"), "SmallArray<short,3>");
        assert_eq!(normalize("long unsigned int"), normalize("unsigned long"));
        assert_eq!(normalize("Pair<long long unsigned int, char>"), "Pair<unsigned long long,char>");
        assert_eq!(normalize("unsigned __int64"), "unsigned long long");
        assert_eq!(normalize("unsigned"), "unsigned int");
    }

    #[test]
    fn identifiers_containing_keywords_are_untouched() {
        assert_eq!(normalize("long_name::shortcut<int_t>"), "long_name::shortcut<int_t>");
        assert_eq!(normalize("engine::Outer::Inner"), "engine::Outer::Inner");
    }
}
