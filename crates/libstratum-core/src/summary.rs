//! Summary lens: every byte of the image attributed exactly once, per address space
//! (docs/12-m3-symbols-sizes.md §3, ADR-0022).
//!
//! Per space: the *universe* is the union of segment and section extents (file space: the whole
//! input). Sections partition it; bytes no section owns are named gaps. Within a section, sized
//! symbols partition the bytes they cover: identical intervals are one alias group, nested
//! intervals belong to the outermost, partial overlaps go to the earlier start with a diagnostic.

use std::collections::{BTreeMap, HashMap};

use libstratum_model::{
    BerkeleySizes, DiagCode, Diagnostic, Dimension, Evidence, Severity, Size, Summary, SummaryRow, SymbolEntry,
    SymbolKind,
};

use crate::ir::{AddrRange, Extent, Section, SectionKind};
use crate::spi::Image;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Space {
    Vm,
    File,
    Load,
}

const SPACES: [Space; 3] = [Space::Vm, Space::File, Space::Load];

fn extent_in(extent: &Extent, space: Space) -> Option<AddrRange> {
    match space {
        Space::Vm => extent.vm,
        Space::File => extent.file,
        Space::Load => extent.load,
    }
    .filter(|r| r.size > 0)
}

fn add(size: &mut Size, space: Space, bytes: u64) {
    match space {
        Space::Vm => size.vm = size.vm.saturating_add(bytes),
        Space::File => size.file = size.file.saturating_add(bytes),
        Space::Load => size.load = size.load.saturating_add(bytes),
    }
}

/// Half-open interval `[start, end)`; `end` saturates so hostile headers can't overflow.
fn bounds(r: AddrRange) -> (u64, u64) {
    (r.start, r.start.saturating_add(r.size))
}

/// Zero-initialized TLS (`.tbss`) is a template, not memory at its address: ELF gives it the address
/// of the following section. It yields to overlapping sections without a diagnostic.
fn is_tls_template(section: &Section) -> bool {
    section.kind == SectionKind::Tls && section.extent.file.is_none()
}

fn section_label(section: &Section) -> String {
    match &section.segment {
        Some(segment) => format!("{segment},{}", section.name),
        None => section.name.clone(),
    }
}

/// An alias group: sized symbols with an identical interval in one section.
struct Group {
    section: usize,
    address: u64,
    size: u64,
    name: String,
    members: usize,
    evidence: Vec<Evidence>,
}

/// Who owns a span of bytes.
#[derive(Clone, Copy)]
enum Owner {
    Gap(&'static str),
    NamedSegment(usize),
    Symbol(usize),
    NoSymbol(usize),
}

pub(crate) fn build(image: &dyn Image, file_size: u64, symbols: &[SymbolEntry], dimensions: &[Dimension]) -> Summary {
    let dimensions: Vec<Dimension> = if dimensions.is_empty() { vec![Dimension::Section] } else { dimensions.to_vec() };
    let sections = image.sections();
    let segments = image.segments();
    let groups = groups(sections, symbols);

    let mut total = Size::default();
    let mut shared = Size::default();
    let mut diagnostics = DiagSet::new();
    // (unattributed, path) → (size, evidence)
    let mut table: BTreeMap<(bool, Vec<String>), (Size, Vec<Evidence>)> = BTreeMap::new();

    for space in SPACES {
        let mut spans: Vec<(Owner, u64)> = Vec::new();
        total_and_spans(space, image, file_size, &groups, &mut spans, &mut diagnostics, &mut total);
        for (owner, bytes) in spans {
            if let Owner::Symbol(g) = owner
                && groups[g].members > 1
            {
                add(&mut shared, space, bytes);
            }
            let (unattributed, path, evidence) = row_key(owner, &dimensions, sections, segments, &groups);
            let entry = table.entry((unattributed, path)).or_default();
            add(&mut entry.0, space, bytes);
            for e in evidence {
                if !entry.1.contains(&e) {
                    entry.1.push(e);
                }
            }
        }
    }

    let (mut rows, mut unattributed) = (Vec::new(), Vec::new());
    for ((is_gap, path), (size, evidence)) in table {
        let row = SummaryRow { path, size, evidence };
        if is_gap { unattributed.push(row) } else { rows.push(row) }
    }
    let diagnostics = diagnostics
        .into_iter()
        .map(|(message, (code, subject))| Diagnostic { severity: Severity::Warning, code, message, subject })
        .collect();
    Summary { dimensions, total, rows, unattributed, shared, diagnostics }
}

/// message → (code, subject); keyed by message so repeated findings across spaces collapse.
type DiagSet = BTreeMap<String, (DiagCode, Option<String>)>;

fn total_and_spans(
    space: Space,
    image: &dyn Image,
    file_size: u64,
    groups: &[Group],
    spans: &mut Vec<(Owner, u64)>,
    diagnostics: &mut DiagSet,
    total: &mut Size,
) {
    let sections = image.sections();
    let segments = image.segments();

    // Universe. A mapping with no access (Mach-O `__PAGEZERO`) reserves address space but occupies none.
    let mut universe: Vec<(u64, u64)> = Vec::new();
    if space == Space::File {
        universe.push((0, file_size));
    }
    for segment in segments {
        if space == Space::Vm && segment.access.is_none() {
            continue;
        }
        universe.extend(extent_in(&segment.extent, space).map(bounds));
    }
    universe.extend(sections.iter().filter_map(|s| extent_in(&s.extent, space)).map(bounds));
    let universe = merge(universe);
    add(total, space, universe.iter().fold(0u64, |n, (s, e)| n.saturating_add(e - s)));

    // Sections: sorted by start, outermost first, TLS templates last among equals.
    let mut claims: Vec<(u64, u64, bool, usize)> = sections
        .iter()
        .enumerate()
        .filter_map(|(i, s)| extent_in(&s.extent, space).map(|r| (bounds(r), is_tls_template(s), i)))
        .map(|((start, end), tls, i)| (start, end, tls, i))
        .collect();
    claims.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)).then(b.1.cmp(&a.1)).then(a.3.cmp(&b.3)));
    let mut owned: Vec<(u64, u64, usize)> = Vec::new();
    let mut claimed_end = 0u64;
    let mut claimed_by: Option<usize> = None;
    for (start, end, tls, i) in claims {
        let from = start.max(claimed_end);
        if start < claimed_end
            && end > claimed_end
            && !tls
            && claimed_by.is_some_and(|c| !is_tls_template(&sections[c]))
        {
            let other = claimed_by.map(|c| section_label(&sections[c])).unwrap_or_default();
            diagnostics.insert(
                format!(
                    "section {} overlaps {other}; the earlier section owns the shared bytes",
                    section_label(&sections[i])
                ),
                (DiagCode::SectionOverlap, Some(section_label(&sections[i]))),
            );
        }
        if from < end {
            owned.push((from, end, i));
            claimed_end = end;
            claimed_by = Some(i);
        }
    }

    // Symbols inside each owned section span.
    for &(from, to, i) in &owned {
        let section = &sections[i];
        let (Some(vm), Some(here)) = (section.extent.vm, extent_in(&section.extent, space)) else {
            spans.push((Owner::NoSymbol(i), to - from));
            continue;
        };
        let mut members: Vec<(u64, u64, usize)> = groups
            .iter()
            .enumerate()
            .filter(|(_, g)| g.section == i)
            .filter_map(|(g, group)| {
                let start = here.start.checked_add(group.address.checked_sub(vm.start)?)?;
                let end = start.saturating_add(group.size);
                let (start, end) = (start.max(from), end.min(to));
                (start < end).then_some((start, end, g))
            })
            .collect();
        members.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        let mut cursor = from;
        let mut owner: Option<usize> = None;
        for (start, end, g) in members {
            if start > cursor {
                spans.push((Owner::NoSymbol(i), start - cursor));
                cursor = start;
            }
            if start < cursor && end > cursor && space == Space::Vm {
                let other = owner.map(|o| groups[o].name.clone()).unwrap_or_default();
                diagnostics.insert(
                    format!("symbol {} overlaps {other}; the earlier symbol owns the shared bytes", groups[g].name),
                    (DiagCode::SymbolOverlap, Some(groups[g].name.clone())),
                );
            }
            if end > cursor {
                spans.push((Owner::Symbol(g), end - cursor));
                cursor = end;
                owner = Some(g);
            }
        }
        if cursor < to {
            spans.push((Owner::NoSymbol(i), to - cursor));
        }
    }

    // Gaps: universe minus owned section spans.
    let lowest = universe.first().map_or(0, |u| u.0);
    let file_segments: Vec<(u64, u64)> =
        segments.iter().filter_map(|seg| extent_in(&seg.extent, Space::File)).map(bounds).collect();
    let last_section_end = owned.iter().map(|o| o.1).max().unwrap_or(0);
    let mut owned_sorted: Vec<(u64, u64)> = owned.iter().map(|&(s, e, _)| (s, e)).collect();
    owned_sorted.sort_unstable();
    let named_empty: Vec<(u64, u64, usize)> = segments
        .iter()
        .enumerate()
        .filter(|(_, seg)| seg.name.is_some())
        .filter(|(_, seg)| !(space == Space::Vm && seg.access.is_none()))
        .filter_map(|(k, seg)| extent_in(&seg.extent, space).map(|r| (bounds(r), k, seg)))
        .filter(|((s, e), _, _)| !owned_sorted.iter().any(|&(os, oe)| os < *e && oe > *s))
        .map(|((s, e), k, _)| (s, e, k))
        .collect();
    for &(ustart, uend) in &universe {
        let mut cursor = ustart;
        let mut pieces = Vec::new();
        for &(s, e) in owned_sorted.iter().filter(|(s, e)| *e > ustart && *s < uend) {
            if s > cursor {
                pieces.push((cursor, s));
            }
            cursor = cursor.max(e);
        }
        if cursor < uend {
            pieces.push((cursor, uend));
        }
        for (gap_start, gap_end) in pieces {
            // Split at section-less named segments (Mach-O `__LINKEDIT`) so they're reported by name.
            let mut cuts: Vec<u64> =
                named_empty.iter().flat_map(|&(s, e, _)| [s, e]).filter(|&p| p > gap_start && p < gap_end).collect();
            // Container headers occupy the start of the file and, where a segment maps file offset 0,
            // the start of that segment; the rest of such a gap is padding.
            let headers_end = match space {
                Space::File => Some(image.headers_size()),
                _ => segments
                    .iter()
                    .filter(|seg| seg.extent.file.is_some_and(|f| f.start == 0))
                    .filter_map(|seg| extent_in(&seg.extent, space))
                    .find(|r| r.start == gap_start)
                    .map(|r| r.start.saturating_add(image.headers_size())),
            }
            .filter(|_| gap_start == lowest);
            cuts.extend(headers_end.filter(|&h| h > gap_start && h < gap_end));
            cuts.push(gap_start);
            cuts.push(gap_end);
            cuts.sort_unstable();
            cuts.dedup();
            for w in cuts.windows(2) {
                let (s, e) = (w[0], w[1]);
                let owner = if let Some(&(_, _, k)) = named_empty.iter().find(|(ns, ne, _)| *ns <= s && e <= *ne) {
                    Owner::NamedSegment(k)
                } else if headers_end.is_some_and(|h| s == gap_start && h > s) {
                    Owner::Gap("[headers]")
                } else if space == Space::File
                    && s >= last_section_end
                    && !file_segments.iter().any(|&(fs, fe)| fs <= s && e <= fe)
                {
                    Owner::Gap("[non-section]")
                } else {
                    Owner::Gap("[alignment]")
                };
                spans.push((owner, e - s));
            }
        }
    }
}

fn merge(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.retain(|(s, e)| s < e);
    ranges.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (s, e) in ranges {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

fn groups(sections: &[Section], symbols: &[SymbolEntry]) -> Vec<Group> {
    let mut by_key: HashMap<(Option<&str>, &str), Vec<usize>> = HashMap::new();
    for (i, s) in sections.iter().enumerate() {
        by_key.entry((s.segment.as_deref(), s.name.as_str())).or_default().push(i);
    }
    let mut grouped: BTreeMap<(usize, u64, u64), Vec<&SymbolEntry>> = BTreeMap::new();
    for symbol in symbols {
        // TLS symbol values are offsets into the TLS block, not addresses.
        let (Some(key), Some(size)) = (&symbol.section, symbol.size) else { continue };
        if size == 0 || symbol.kind == SymbolKind::Tls {
            continue;
        }
        let Some(candidates) = by_key.get(&(key.segment.as_deref(), key.name.as_str())) else { continue };
        let Some(&section) =
            candidates.iter().find(|&&i| sections[i].extent.vm.is_some_and(|vm| vm.contains(symbol.address)))
        else {
            continue;
        };
        grouped.entry((section, symbol.address, size)).or_default().push(symbol);
    }
    grouped
        .into_iter()
        .map(|((section, address, size), mut members)| {
            members.sort_by(|a, b| a.raw_name.cmp(&b.raw_name));
            let mut evidence: Vec<Evidence> = Vec::new();
            for e in members.iter().filter_map(|m| m.size_evidence.clone()) {
                if !evidence.contains(&e) {
                    evidence.push(e);
                }
            }
            let first = members[0];
            Group {
                section,
                address,
                size,
                name: first.demangled.clone().unwrap_or_else(|| first.raw_name.clone()),
                members: members.len(),
                evidence,
            }
        })
        .collect()
}

fn computed(rule: &str) -> Evidence {
    Evidence::Computed { rule: rule.into() }
}

/// Maps an owner to its row: `(unattributed, path, evidence)`.
fn row_key(
    owner: Owner,
    dimensions: &[Dimension],
    sections: &[Section],
    segments: &[crate::ir::Segment],
    groups: &[Group],
) -> (bool, Vec<String>, Vec<Evidence>) {
    let has = |d: Dimension| dimensions.contains(&d);
    let (section, symbol, evidence) = match owner {
        Owner::Gap(label) => return (true, vec![label.to_owned()], vec![computed("bytes no section covers")]),
        Owner::NamedSegment(k) => {
            let name = segments[k].name.clone().unwrap_or_default();
            return (true, vec![format!("[{name}]")], vec![computed("segment without sections")]);
        }
        Owner::NoSymbol(i) => {
            if has(Dimension::Symbol) {
                let path = dimensions
                    .iter()
                    .map(|d| match d {
                        Dimension::Section => section_label(&sections[i]),
                        _ => "[no symbol]".to_owned(),
                    })
                    .collect();
                return (true, path, vec![computed("bytes no symbol covers")]);
            }
            (i, None, vec![computed("section headers")])
        }
        Owner::Symbol(g) => {
            let evidence =
                if has(Dimension::Symbol) { groups[g].evidence.clone() } else { vec![computed("section headers")] };
            (groups[g].section, Some(g), evidence)
        }
    };
    let path = dimensions
        .iter()
        .map(|d| match d {
            Dimension::Section => section_label(&sections[section]),
            _ => symbol.map_or_else(|| "[no symbol]".to_owned(), |g| groups[g].name.clone()),
        })
        .collect();
    (false, path, evidence)
}

/// Berkeley `size` totals over allocated (VM-mapped) sections.
pub(crate) fn berkeley(image: &dyn Image) -> BerkeleySizes {
    let mut out = BerkeleySizes::default();
    for section in image.sections() {
        let Some(vm) = section.extent.vm else { continue };
        if !section.access.write || section.access.execute {
            out.text = out.text.saturating_add(vm.size);
        } else if section.extent.file.is_some() {
            out.data = out.data.saturating_add(vm.size);
        } else {
            out.bss = out.bss.saturating_add(vm.size);
        }
    }
    out
}
