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
        RawLayoutEntry::Field { name, type_name, offset_bits, size_bits, align_bytes, .. } => LayoutMember {
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

fn entry_offset(entry: &RawLayoutEntry) -> u64 {
    match entry {
        RawLayoutEntry::Field { offset_bits, .. }
        | RawLayoutEntry::Bitfield { offset_bits, .. }
        | RawLayoutEntry::VtablePtr { offset_bits, .. } => *offset_bits,
        RawLayoutEntry::Base { offset_bits, .. } => offset_bits.unwrap_or(u64::MAX),
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
    // Data members sharing storage in a struct: an anonymous union whose members the debug format lists
    // directly (MSVC/clang-cl PDBs don't record the union at all). Invariant: two non-empty data members
    // (fields or bitfields) whose bit ranges intersect. Empty-class members are excluded because they may
    // share an address by design; bitfields in one storage unit occupy disjoint bits and never match.
    if kind != AggregateKind::Union {
        let data: Vec<(usize, u64, u64)> = raw
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                RawLayoutEntry::Field { offset_bits, size_bits, empty_type: false, .. } if *size_bits > 0 => {
                    Some((i, *offset_bits, *size_bits))
                }
                RawLayoutEntry::Bitfield { offset_bits, width_bits, .. } if *width_bits > 0 => {
                    Some((i, *offset_bits, u64::from(*width_bits)))
                }
                _ => None,
            })
            .collect();
        let mut overlapping: Vec<usize> = data
            .iter()
            .filter(|(i, start, size)| {
                data.iter().any(|(j, other_start, other_size)| {
                    i != j
                        && *start < other_start.saturating_add(*other_size)
                        && *other_start < start.saturating_add(*size)
                })
            })
            .map(|(i, ..)| *i)
            .collect();
        overlapping.sort_by_key(|i| raw.entries.get(*i).map(entry_offset));
        if !overlapping.is_empty() {
            let names: Vec<String> = overlapping.iter().map(|&i| display_name(&member(&raw.entries[i]))).collect();
            notes.push(format!(
                "members {} share storage (an anonymous union flattened by the debug format)",
                names.join(", ")
            ));
        }
    }
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

/// Suggests a member order with the smallest size reachable by reordering, or `None` if reordering
/// can't shrink the type or the result couldn't be proven minimal.
///
/// # Why the result is minimal
///
/// Preconditions, checked below; if any fails no suggestion is made:
/// 1. every member alignment is a power of two;
/// 2. every member size is a multiple of its alignment (true for every complete C/C++ type);
/// 3. no bitfields, bases, vtable pointers or packing (the caller only passes plain fields of
///    non-packed structs).
///
/// Let `A` be the struct alignment and `S` the sum of member sizes.
/// * **Lower bound.** Members don't overlap and the size is a multiple of `A`, so every ordering has
///   size `>= round_up(S, A)`. A flexible array member (size 0) must stay last; its offset is at
///   least `round_up(S, a_fam)` in any ordering.
/// * **The greedy order reaches it.** Order by decreasing alignment. When member `k` is placed, every
///   earlier member `j` has `a_j >= a_k`; alignments are powers of two, so `a_k | a_j | s_j`, and the
///   running offset `sum(s_j)` is already a multiple of `a_k`: no padding is inserted before any
///   member. The final size is exactly `round_up(S, A)` (the flexible array member, kept last, lands
///   at `round_up(S, a_fam)`), i.e. the lower bound.
///
/// The claim is relative to the member alignments the lens has. They come from the debug info when
/// it records them (`DW_AT_alignment`) and otherwise from the ABI rule that a scalar aligns to its
/// size; the caveats say so.
fn suggest_reorder(members: &[LayoutMember], size_bytes: u64, alignment: Option<u64>) -> Option<ReorderSuggestion> {
    let alignment = alignment?;
    let mut fields: Vec<(&LayoutMember, u64, u64)> =
        members.iter().map(|m| m.align_bytes.map(|a| (m, a.max(1), m.size_bits.div_ceil(8)))).collect::<Option<_>>()?;
    let preconditions_hold = alignment.is_power_of_two()
        && fields.iter().all(|(m, align, size)| align.is_power_of_two() && size % align == 0 && m.size_bits % 8 == 0);
    if !preconditions_hold {
        return None;
    }

    // A trailing zero-size member is a flexible array member: C requires it to stay last.
    let last_by_offset = members.iter().max_by_key(|m| m.offset_bits.unwrap_or(0));
    let flexible = last_by_offset.filter(|m| m.size_bits == 0);
    fields.sort_by(|a, b| {
        let a_flex = flexible.is_some_and(|f| std::ptr::eq(f, a.0));
        let b_flex = flexible.is_some_and(|f| std::ptr::eq(f, b.0));
        a_flex.cmp(&b_flex).then(b.1.cmp(&a.1)).then(b.2.cmp(&a.2))
    });

    let new_size = packed_size(fields.iter().map(|(_, align, size)| (*align, *size)), alignment);
    if new_size >= size_bytes {
        return None;
    }
    Some(ReorderSuggestion {
        order: fields.iter().map(|(m, ..)| display_name(m)).collect(),
        new_size_bytes: new_size,
        saved_bytes: size_bytes - new_size,
        caveats: vec![
            "smallest size reachable by reordering these members, given the member alignments shown".into(),
            "changes the type's ABI and any serialized representation".into(),
            "consider access locality: fields used together should stay close".into(),
        ],
    })
}

/// Size of a struct whose members are laid out in the given order: each member at the next offset
/// that is a multiple of its alignment, total rounded up to the struct alignment.
fn packed_size(members: impl IntoIterator<Item = (u64, u64)>, struct_align: u64) -> u64 {
    let mut offset = 0u64;
    for (align, size) in members {
        offset = offset.div_ceil(align) * align + size;
    }
    offset.div_ceil(struct_align) * struct_align
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
            empty_type: false,
        }
    }

    fn bitfield(name: &str, offset_bits: u64, width_bits: u32) -> RawLayoutEntry {
        RawLayoutEntry::Bitfield { name: Some(name.into()), type_name: "t".into(), offset_bits, width_bits }
    }

    fn notes_for(entries: Vec<RawLayoutEntry>, size: u64) -> Vec<String> {
        compute(&raw("S", size, entries), 64).notes
    }

    #[test]
    fn small_members_sharing_storage_are_noted() {
        // struct { char tag; union { char a; char b; }; } flattened by a PDB.
        let notes = notes_for(vec![field("tag", 0, 1, 1), field("a", 1, 1, 1), field("b", 1, 1, 1)], 2);
        assert_eq!(notes, ["members a, b share storage (an anonymous union flattened by the debug format)"]);

        // struct { union { char c; unsigned bits : 3; }; }: a char overlapping a bitfield.
        let notes = notes_for(vec![field("c", 0, 1, 1), bitfield("bits", 0, 3)], 4);
        assert_eq!(notes, ["members c, bits share storage (an anonymous union flattened by the debug format)"]);

        // Partial overlap, not just equal offsets: short at 0 and char at 1.
        let notes = notes_for(vec![field("s", 0, 2, 2), field("c", 1, 1, 1)], 2);
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn legitimate_address_sharing_is_not_noted() {
        // [[no_unique_address]] empty member at the same address as a char.
        let empty = RawLayoutEntry::Field {
            name: Some("alloc".into()),
            type_name: "Empty".into(),
            offset_bits: 0,
            size_bits: 8,
            align_bytes: Some(1),
            empty_type: true,
        };
        assert!(notes_for(vec![empty, field("c", 0, 1, 1)], 1).is_empty());

        // Adjacent bitfields in one storage unit, and a zero-size flexible array member.
        assert!(notes_for(vec![bitfield("a", 0, 3), bitfield("b", 3, 5), field("data", 8, 0, 4)], 4).is_empty());
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

    /// The greedy order equals the minimum over every permutation, for random member mixes that
    /// satisfy the documented preconditions (power-of-two alignments, sizes multiple of alignment).
    #[test]
    fn reorder_suggestion_is_minimal_over_all_permutations() {
        fn permutations(items: &mut Vec<(u64, u64)>, k: usize, best: &mut u64, align: u64) {
            if k == items.len() {
                *best = (*best).min(packed_size(items.iter().copied(), align));
                return;
            }
            for i in k..items.len() {
                items.swap(k, i);
                permutations(items, k + 1, best, align);
                items.swap(k, i);
            }
        }
        // Deterministic xorshift so failures reproduce.
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2000 {
            let n = 2 + (next() % 6) as usize; // 2..=7 members
            let members: Vec<(u64, u64)> = (0..n)
                .map(|_| {
                    let align = 1u64 << (next() % 5); // 1..16
                    (align, align * (1 + next() % 3))
                })
                .collect();
            let max_align = members.iter().map(|m| m.0).max().unwrap();
            let struct_align = max_align << (next() % 2); // sometimes an explicit larger alignas
            let mut best = u64::MAX;
            permutations(&mut members.clone(), 0, &mut best, struct_align);

            let mut greedy = members.clone();
            greedy.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            assert_eq!(packed_size(greedy.iter().copied(), struct_align), best, "{members:?} align {struct_align}");
        }
    }

    #[test]
    fn flexible_array_member_stays_last() {
        // struct { char c; double d; int n; int data[]; }: FAM aligns to 4 and must remain last.
        let layout = compute(
            &raw(
                "Msg",
                24,
                vec![field("c", 0, 1, 1), field("d", 8, 8, 8), field("n", 16, 4, 4), field("data", 20, 0, 4)],
            ),
            64,
        );
        let s = layout.suggestion.unwrap();
        assert_eq!(s.order.last().map(String::as_str), Some("data"));
        assert_eq!(s.new_size_bytes, 16);
    }

    #[test]
    fn no_suggestion_when_preconditions_fail() {
        // A size that isn't a multiple of its alignment can't come from a complete C/C++ type:
        // the optimality proof doesn't apply, so nothing is claimed.
        let layout = compute(&raw("Odd", 24, vec![field("a", 0, 1, 1), field("b", 8, 12, 8)]), 64);
        assert!(layout.suggestion.is_none());
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
