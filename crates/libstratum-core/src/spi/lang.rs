use std::fmt::Debug;

use crate::ir::SourceLanguage;

/// Language semantics: which units to correlate and how to normalize type names.
pub trait LanguageSupport: Send + Sync + Debug + 'static {
    /// Stable id: "c-cpp".
    fn id(&self) -> &'static str;

    fn claims(&self, language: SourceLanguage) -> bool;

    /// Normalizes a qualified type name so spellings from different toolchains
    /// compare equal (e.g. `std::vector<int,std::allocator<int> >` vs Clang's form).
    fn normalize_type_name(&self, name: &str) -> String;
}
