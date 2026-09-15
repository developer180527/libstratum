//! Architecture facts shared by plugins and the core.

use libstratum_model::Arch;

/// Pointer size in bytes, if fixed for the architecture.
pub fn pointer_size(arch: Arch) -> Option<u8> {
    match arch {
        Arch::X86_64 | Arch::Aarch64 => Some(8),
        Arch::Arm | Arch::Riscv32 => Some(4),
        _ => None,
    }
}

/// Splits an Arm ELF symbol value into `(address, is_thumb)`.
/// Thumb function symbols have bit 0 set; debug info addresses do not (docs/10 §2.1).
pub fn split_thumb_bit(arch: Arch, value: u64, is_function: bool) -> (u64, bool) {
    if arch == Arch::Arm && is_function && value & 1 == 1 { (value & !1, true) } else { (value, false) }
}

/// Arm and AArch64 mapping symbols (`$a`, `$t`, `$d`, `$x`, optionally with a `.suffix`).
/// They mark code/data regions and must not be reported as program symbols.
pub fn is_mapping_symbol(arch: Arch, name: &str) -> bool {
    if !matches!(arch, Arch::Arm | Arch::Aarch64) {
        return false;
    }
    let base = name.split('.').next().unwrap_or(name);
    matches!(base, "$a" | "$t" | "$d" | "$x")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_bit_is_cleared_only_for_arm_functions() {
        assert_eq!(split_thumb_bit(Arch::Arm, 0x0800_0401, true), (0x0800_0400, true));
        assert_eq!(split_thumb_bit(Arch::Arm, 0x2000_0001, false), (0x2000_0001, false));
        assert_eq!(split_thumb_bit(Arch::Aarch64, 0x1001, true), (0x1001, false));
    }

    #[test]
    fn mapping_symbols() {
        assert!(is_mapping_symbol(Arch::Arm, "$t"));
        assert!(is_mapping_symbol(Arch::Arm, "$d.realdata"));
        assert!(!is_mapping_symbol(Arch::Arm, "main"));
        assert!(!is_mapping_symbol(Arch::X86_64, "$t"));
    }
}
