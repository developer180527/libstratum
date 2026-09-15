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
        // TODO(M2): full cross-toolchain normalization (docs/11 §2.5).
        // For now only unify MSVC's `> >` template spacing with Clang/GCC's `>>`.
        name.replace("> >", ">>").replace(", ", ",")
    }
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
        let msvc = CCpp.normalize_type_name("std::vector<int,std::allocator<int> >");
        let clang = CCpp.normalize_type_name("std::vector<int, std::allocator<int>>");
        assert_eq!(msvc, clang);
    }
}
