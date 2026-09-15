use std::fmt::Debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameStyle {
    /// Full signature including template arguments and parameters.
    Full,
    /// Without template arguments and parameters; groups instantiations.
    Short,
}

/// One mangling scheme (Itanium, MSVC). Separate from language semantics:
/// clang-cl emits MSVC mangling for C++ (docs/11 §2.5).
pub trait Demangler: Send + Sync + Debug + 'static {
    /// Stable id: "itanium", "msvc".
    fn id(&self) -> &'static str;

    /// Cheap check whether `raw` uses this scheme.
    fn recognizes(&self, raw: &str) -> bool;

    fn demangle(&self, raw: &str, style: NameStyle) -> Option<String>;
}
