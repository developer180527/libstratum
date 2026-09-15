//! Layout lens: turns backend `RawLayout`s into public `TypeLayout`s (docs/05 §2).
//! Derived facts (holes, padding, packing, cache lines, reorder suggestions) are computed here,
//! once, for every debug-info backend.

use libstratum_model::{
    AggregateKind, CachelineView, Evidence, Hole, LayoutMember, MemberKind, ReorderSuggestion, TypeLayout,
};

use crate::ir::{self, RawLayout, RawLayoutEntry};

pub const DEFAULT_CACHELINE_BYTES: u64 = 64;

fn member(entry: &RawLayoutEntry) -> LayoutMember {
    match entry {
        RawLayoutEntry::Field { name, type_name, offset_bits, size_bits, align_bytes } => LayoutMember {
            kind: MemberKind::Field,
            name: name.clone(),
            type_name: Some(type_name.clone()),
            offset_bits: Some(*offset_bits),
            size_bits: *size_bits,
            align_bytes: *align_bytes,
        },
        RawLayoutEntry::Bitfield { name, type_name, offset_bits, width_bits } => LayoutMember {
            kind: MemberKind::Bitfield,
            name: name.clone(),
            type_name: Some(type_name.clone()),
            offset_bits: Some(*offset_bits),
            size_bits: u64::from(*width_bits),
            align_bytes: None,
        },
        RawLayoutEntry::Base { type_name, offset_bits, size_bits, is_virtual } => LayoutMember {
            kind: if *is_virtual { MemberKind::VirtualBase } else { MemberKind::Base },
            name: None,
            type_name: Some(type_name.clone()),
            offset_bits: *offset_bits,
            size_bits: *size_bits,
            align_bytes: None,
        },
        RawLayoutEntry::VtablePtr { offset_bits, size_bits } => LayoutMember {
            kind: MemberKind::VtablePtr,
            name: None,
            type_name: None,
            offset_bits: Some(*offset_bits),
            size_bits: *size_bits,
            align_bytes: Some(size_bits / 8),
        },
    }
}

fn display_name(m: &LayoutMember) -> String {
    match (m.kind, &m.name, &m.type_name) {
        (_, Some(name), _) => name.clone(),
        (MemberKind::VtablePtr, ..) => "<vtable pointer>".into(),
        (MemberKind::Base | MemberKind::VirtualBase, _, Some(ty)) => format!("<base {ty}>"),
        (_, None, Some(ty)) => format!("<anonymous {ty}>"),
        _ => "<anonymous>".into(),
    }
}

/// Builds the public layout for one raw definition.
pub fn compute(raw: &RawLayout, cacheline_bytes: u64) -> TypeLayout {
    let mut members: Vec<LayoutMember> = raw.entries.iter().map(member).collect();
    // Stable sort by offset; virtual bases (unknown offset) last.
    members.sort_by_key(|m| m.offset_bits.unwrap_or(u64::MAX));

    let size_bits = raw.byte_size.saturating_mul(8);
    let kind = match raw.kind {
        ir::AggregateKind::Struct => AggregateKind::Struct,
        ir::AggregateKind::Class => AggregateKind::Class,
        ir::AggregateKind::Union => AggregateKind::Union,
    };

    // Holes: walk placed members; overlaps (unions, EBO, no_unique_address) just extend the cursor.
    // With virtual bases, compilers place hidden members (virtual base subobjects; MSVC's implicit
    // vbptr) that no member record describes, so gaps can't be classified as padding.
    let mut notes = Vec::new();
    let mut holes = Vec::new();
    let mut cursor = 0u64;
    for m in members.iter().filter(|m| m.offset_bits.is_some()) {
        let start = m.offset_bits.unwrap_or(0);
        if kind != AggregateKind::Union && start > cursor && !raw.has_virtual_bases {
            holes.push(Hole { offset_bits: cursor, size_bits: start - cursor });
        }
        cursor = cursor.max(start.saturating_add(m.size_bits));
    }
    let tail_padding_bits = if raw.has_virtual_bases {
        notes.push(
            "has virtual bases: hidden subobjects and virtual-base pointers occupy bytes that no member describes, so holes and padding are not computed"
                .into(),
        );
        0
    } else {
        size_bits.saturating_sub(cursor)
    };
    let padding_bits = holes.iter().map(|h| h.size_bits).sum::<u64>() + tail_padding_bits;

    let fields_only = members.iter().all(|m| m.kind == MemberKind::Field);
    let packed = members.iter().any(|m| {
        m.kind == MemberKind::Field
            && matches!((m.offset_bits, m.align_bytes), (Some(off), Some(align)) if align > 1 && off % (align * 8) != 0)
    });
    let computed_align = if fields_only {
        members.iter().map(|m| m.align_bytes).try_fold(1u64, |acc, a| a.map(|a| acc.max(a)))
    } else {
        None
    };
    let alignment_bytes = raw.alignment.or(if packed { None } else { computed_align });

    let cacheline = cacheline_view(&members, raw.byte_size, cacheline_bytes);
    let suggestion = if kind != AggregateKind::Union && fields_only && !packed {
        suggest_reorder(&members, raw.byte_size, alignment_bytes)
    } else {
        None
    };

    let mut evidence = Vec::new();
    if suggestion.is_some() || (raw.alignment.is_none() && alignment_bytes.is_some()) {
        evidence.push(Evidence::Computed { rule: "alignment = max(member alignment); offsets rounded up".into() });
    }

    TypeLayout {
        name: raw.name.clone(),
        kind,
        size_bytes: raw.byte_size,
        alignment_bytes,
        decl: raw.decl.clone(),
        members,
        holes,
        tail_padding_bits,
        padding_bits,
        packed,
        cacheline,
        suggestion,
        notes,
        evidence,
    }
}

fn cacheline_view(members: &[LayoutMember], size_bytes: u64, cacheline_bytes: u64) -> CachelineView {
    let cacheline_bytes = cacheline_bytes.max(1);
    let boundaries: Vec<u64> = (1..).map(|i| i * cacheline_bytes).take_while(|&b| b < size_bytes).collect();
    let straddling = members
        .iter()
        .filter(|m| {
            let (Some(off), true) = (m.offset_bits, m.size_bits > 0) else { return false };
            let first = off / 8 / cacheline_bytes;
            let last = (off + m.size_bits - 1) / 8 / cacheline_bytes;
            first != last
        })
        .map(display_name)
        .collect();
    CachelineView { cacheline_bytes, boundaries, straddling }
}

/// Largest alignment first (then largest size), recomputing offsets with each member's alignment.
fn suggest_reorder(members: &[LayoutMember], size_bytes: u64, alignment: Option<u64>) -> Option<ReorderSuggestion> {
    let alignment = alignment?;
    let mut fields: Vec<(&LayoutMember, u64, u64)> =
        members.iter().map(|m| m.align_bytes.map(|a| (m, a.max(1), m.size_bits.div_ceil(8)))).collect::<Option<_>>()?;
    fields.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));

    let mut offset = 0u64;
    for (_, align, size) in &fields {
        offset = offset.div_ceil(*align) * align + size;
    }
    let new_size = offset.div_ceil(alignment) * alignment;
    if new_size >= size_bytes {
        return None;
    }
    Some(ReorderSuggestion {
        order: fields.iter().map(|(m, ..)| display_name(m)).collect(),
        new_size_bytes: new_size,
        saved_bytes: size_bytes - new_size,
        caveats: vec![
            "changes the type's ABI and any serialized representation".into(),
            "consider access locality: fields used together should stay close".into(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, off_bytes: u64, size_bytes: u64, align: u64) -> RawLayoutEntry {
        RawLayoutEntry::Field {
            name: Some(name.into()),
            type_name: "t".into(),
            offset_bits: off_bytes * 8,
            size_bits: size_bytes * 8,
            align_bytes: Some(align),
        }
    }

    fn raw(name: &str, size: u64, entries: Vec<RawLayoutEntry>) -> RawLayout {
        RawLayout {
            name: name.into(),
            kind: ir::AggregateKind::Struct,
            byte_size: size,
            alignment: None,
            decl: None,
            is_declaration: false,
            has_virtual_bases: false,
            entries,
        }
    }

    #[test]
    fn packet_holes_tail_padding_and_suggestion() {
        // struct Packet { char tag; double value; int count; short flags; } on LP64
        let layout = compute(
            &raw(
                "Packet",
                24,
                vec![
                    field("tag", 0, 1, 1),
                    field("value", 8, 8, 8),
                    field("count", 16, 4, 4),
                    field("flags", 20, 2, 2),
                ],
            ),
            64,
        );
        assert_eq!(layout.holes, vec![Hole { offset_bits: 8, size_bits: 56 }]);
        assert_eq!(layout.tail_padding_bits, 16);
        assert_eq!(layout.padding_bits, 72);
        assert_eq!(layout.alignment_bytes, Some(8));
        assert!(!layout.packed);
        let s = layout.suggestion.unwrap();
        assert_eq!(s.order, ["value", "count", "flags", "tag"]);
        assert_eq!((s.new_size_bytes, s.saved_bytes), (16, 8));
    }

    #[test]
    fn packed_structs_get_no_suggestion() {
        // #pragma pack(1): struct { char v; int len; short sum; } = 7 bytes
        let layout = compute(
            &raw(
                "WireHeader",
                7,
                vec![field("version", 0, 1, 1), field("length", 1, 4, 4), field("checksum", 5, 2, 2)],
            ),
            64,
        );
        assert!(layout.packed);
        assert!(layout.holes.is_empty() && layout.tail_padding_bits == 0);
        assert!(layout.suggestion.is_none());
    }

    #[test]
    fn cacheline_straddling() {
        let layout = compute(
            &raw(
                "CacheStraddle",
                80,
                vec![
                    field("header", 0, 60, 1),
                    field("hot_a", 60, 4, 4),
                    field("hot_b", 64, 4, 4),
                    field("cold", 72, 8, 8),
                ],
            ),
            64,
        );
        assert_eq!(layout.cacheline.boundaries, vec![64]);
        assert!(layout.cacheline.straddling.is_empty(), "hot_a ends exactly at 64");
        let layout = compute(&raw("S", 72, vec![field("a", 0, 62, 1), field("b", 62, 4, 2)]), 64);
        assert_eq!(layout.cacheline.straddling, ["b"]);
    }
}
