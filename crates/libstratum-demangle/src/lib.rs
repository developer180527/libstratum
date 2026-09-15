//! Mangling schemes: Itanium (GCC, Clang on ELF/Mach-O) and MSVC (cl, clang-cl).

use libstratum_core::spi::{Demangler, NameStyle};

/// Itanium C++ ABI mangling (`_Z…`; Mach-O adds a leading `_`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Itanium;

impl Demangler for Itanium {
    fn id(&self) -> &'static str {
        "itanium"
    }

    fn recognizes(&self, raw: &str) -> bool {
        raw.starts_with("_Z") || raw.starts_with("__Z")
    }

    fn demangle(&self, raw: &str, style: NameStyle) -> Option<String> {
        let raw = raw.strip_prefix('_').filter(|r| r.starts_with("_Z")).unwrap_or(raw);
        let symbol = cpp_demangle::Symbol::new(raw).ok()?;
        let options = match style {
            NameStyle::Full => cpp_demangle::DemangleOptions::new(),
            NameStyle::Short => cpp_demangle::DemangleOptions::new().no_params().no_return_type(),
        };
        symbol.demangle_with_options(&options).ok()
    }
}

/// MSVC decoration (`?name@scope@@…`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Msvc;

impl Demangler for Msvc {
    fn id(&self) -> &'static str {
        "msvc"
    }

    fn recognizes(&self, raw: &str) -> bool {
        raw.starts_with('?')
    }

    fn demangle(&self, raw: &str, style: NameStyle) -> Option<String> {
        let flags = match style {
            NameStyle::Full => msvc_demangler::DemangleFlags::llvm(),
            NameStyle::Short => msvc_demangler::DemangleFlags::NAME_ONLY,
        };
        msvc_demangler::demangle(raw, flags).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn itanium_elf_and_macho_spellings() {
        assert!(Itanium.recognizes("_Z2f1i"));
        assert!(Itanium.recognizes("__Z2f1i"));
        assert_eq!(Itanium.demangle("__Z2f1i", NameStyle::Full).as_deref(), Some("f1(int)"));
        assert_eq!(Itanium.demangle("_Z2f1i", NameStyle::Short).as_deref(), Some("f1"));
    }

    #[test]
    fn msvc_member_function() {
        let raw = "?update@MovementSystem@@QEAAXM@Z";
        assert!(Msvc.recognizes(raw));
        let full = Msvc.demangle(raw, NameStyle::Full).unwrap();
        assert!(full.contains("MovementSystem::update"), "{full}");
    }
}
