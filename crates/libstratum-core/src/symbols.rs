//! Symbol lens: one normalized symbol table per session, with sizes and their evidence
//! (docs/12-m3-symbols-sizes.md §4) and alias/fold groups (§6).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use libstratum_model::{
    Binding, Diagnostic, Evidence, SectionKey, SymbolEntry, SymbolFilter, SymbolKind, SymbolResult,
};

use crate::ir::{self, Section, SectionId, UnitId};
use crate::spi::{DebugReader, Demangler, Image, LanguageSupport, NameStyle};

pub(crate) struct SymbolContext<'a> {
    pub image: &'a dyn Image,
    pub debug: &'a [Box<dyn DebugReader>],
    pub demanglers: &'a [Arc<dyn Demangler>],
    pub languages: &'a [Arc<dyn LanguageSupport>],
}

/// Builds the session's symbol table.
pub(crate) fn build(ctx: &SymbolContext<'_>) -> (Vec<SymbolEntry>, Vec<Diagnostic>) {
    let diagnostics = Vec::new();

    // 1. Raw symbols: the container's table, or the debug info's (linked PE images have none).
    let (raw, from_debug): (Vec<ir::Symbol>, bool) = if !ctx.image.symbols().is_empty() {
        (ctx.image.symbols().to_vec(), false)
    } else {
        let found = ctx.debug.iter().find_map(|reader| reader.symbols().ok().flatten());
        match found {
            Some(symbols) => (symbols, true),
            None => (Vec::new(), false),
        }
    };
    let sections: HashMap<SectionId, &Section> = ctx.image.sections().iter().map(|s| (s.id, s)).collect();

    // 2. Sizes recorded by the source.
    let mut entries: Vec<(ir::Symbol, Option<u64>, Option<Evidence>)> = raw
        .into_iter()
        .filter(|s| s.kind != ir::SymbolKind::Section && !s.raw_name.is_empty())
        .map(|s| {
            let recorded = s.size.filter(|&n| n > 0);
            let evidence = recorded.map(|_| if from_debug { debug_evidence() } else { Evidence::SymbolTable });
            (s, recorded, evidence)
        })
        .collect();

    // 3. Debug info for symbols the table didn't size: function ranges (Mach-O nlist has no sizes) and
    //    variable type sizes (data publics in PDBs, data symbols on Mach-O).
    if entries.iter().any(|(_, size, _)| size.is_none()) {
        let functions = function_sizes(ctx.debug);
        let variables = variable_sizes(ctx.debug);
        for (symbol, size, evidence) in entries.iter_mut() {
            if size.is_some() {
                continue;
            }
            let found = match symbol.kind {
                ir::SymbolKind::Function => functions.get(&symbol.address),
                ir::SymbolKind::Object | ir::SymbolKind::Unknown => variables.get(&symbol.address),
                _ => None,
            };
            if let Some(&n) = found.filter(|&&n| n > 0) {
                *size = Some(n);
                *evidence = Some(debug_evidence());
            }
        }
    }

    // 4. Labeled heuristic: distance to the next distinct symbol address in the same section, never past its end.
    let mut by_section: BTreeMap<SectionId, Vec<u64>> = BTreeMap::new();
    for (symbol, ..) in &entries {
        if let Some(section) = symbol.section {
            by_section.entry(section).or_default().push(symbol.address);
        }
    }
    for addresses in by_section.values_mut() {
        addresses.sort_unstable();
        addresses.dedup();
    }
    for (symbol, size, evidence) in entries.iter_mut() {
        if size.is_some() || !matches!(symbol.kind, ir::SymbolKind::Function | ir::SymbolKind::Object) {
            continue;
        }
        let (Some(section_id), Some(section)) = (symbol.section, symbol.section.and_then(|id| sections.get(&id)))
        else {
            continue;
        };
        let Some(vm) = section.extent.vm else { continue };
        let next = by_section[&section_id].iter().copied().find(|&a| a > symbol.address).unwrap_or(vm.end());
        let end = next.min(vm.end());
        if end > symbol.address {
            *size = Some(end - symbol.address);
            *evidence = Some(Evidence::Heuristic { rule: "distance to the next symbol in the section".into() });
        }
    }

    // 5. Alias groups: sized functions and objects with identical (section, address, size).
    let mut groups: HashMap<(Option<SectionId>, u64, u64), Vec<usize>> = HashMap::new();
    for (i, (symbol, size, _)) in entries.iter().enumerate() {
        if let Some(n) = size
            && matches!(symbol.kind, ir::SymbolKind::Function | ir::SymbolKind::Object)
        {
            groups.entry((symbol.section, symbol.address, *n)).or_default().push(i);
        }
    }

    let demangle =
        |raw: &str, style| ctx.demanglers.iter().find(|d| d.recognizes(raw)).and_then(|d| d.demangle(raw, style));
    let mut out: Vec<SymbolEntry> = entries
        .iter()
        .map(|(symbol, size, evidence)| SymbolEntry {
            raw_name: symbol.raw_name.clone(),
            demangled: demangle(&symbol.raw_name, NameStyle::Full),
            kind: match symbol.kind {
                ir::SymbolKind::Function => SymbolKind::Function,
                ir::SymbolKind::Object => SymbolKind::Object,
                ir::SymbolKind::Tls => SymbolKind::Tls,
                ir::SymbolKind::Label => SymbolKind::Label,
                _ => SymbolKind::Unknown,
            },
            binding: match symbol.binding {
                ir::Binding::Global => Binding::Global,
                ir::Binding::Local => Binding::Local,
                ir::Binding::Weak => Binding::Weak,
            },
            section: symbol
                .section
                .and_then(|id| sections.get(&id))
                .map(|s| SectionKey { segment: s.segment.clone(), name: s.name.clone() }),
            address: symbol.address,
            size: *size,
            size_evidence: evidence.clone(),
            aliases: Vec::new(),
            folded: false,
            unit: None,
        })
        .collect();

    for members in groups.values().filter(|m| m.len() > 1) {
        // Distinct functions: names that still differ once demangled and normalized (C1/C2 constructor
        // variants demangle to the same name and are not a fold).
        let mut distinct: Vec<String> = members
            .iter()
            .map(|&i| {
                let name = out[i].demangled.clone().unwrap_or_else(|| out[i].raw_name.clone());
                ctx.languages.first().map_or(name.clone(), |l| l.normalize_type_name(&name))
            })
            .collect();
        distinct.sort();
        distinct.dedup();
        let folded = distinct.len() > 1;
        for &i in members {
            let mut aliases: Vec<String> =
                members.iter().filter(|&&j| j != i).map(|&j| out[j].raw_name.clone()).collect();
            aliases.sort();
            out[i].aliases = aliases;
            out[i].folded = folded;
        }
    }

    out.sort_by(|a, b| a.address.cmp(&b.address).then(a.raw_name.cmp(&b.raw_name)));
    (out, diagnostics)
}

fn debug_evidence() -> Evidence {
    Evidence::DebugInfo { backend: "debug-info".into(), unit: 0, key: None }
}

/// Function start address → size, from debug readers that record final addresses.
fn function_sizes(debug: &[Box<dyn DebugReader>]) -> HashMap<u64, u64> {
    let mut sizes = HashMap::new();
    for reader in debug {
        let Ok(units) = reader.units() else { continue };
        for unit in units {
            let Ok(functions) = reader.functions(UnitId(unit.id.0)) else { continue };
            for function in functions {
                let Some(start) = function.ranges.iter().map(|r| r.start).min() else { continue };
                // The symbol covers the entry range; cold splits are separate ranges.
                if let Some(entry) = function.ranges.iter().find(|r| r.start == start) {
                    sizes.entry(start).or_insert(entry.size);
                }
            }
        }
    }
    sizes
}

/// Variable address → type size, from debug readers that record final addresses.
fn variable_sizes(debug: &[Box<dyn DebugReader>]) -> HashMap<u64, u64> {
    let mut sizes = HashMap::new();
    for reader in debug {
        if let Ok(variables) = reader.variables() {
            for variable in variables {
                sizes.entry(variable.address).or_insert(variable.size);
            }
        }
    }
    sizes
}

/// Applies a filter; name matching accepts raw, demangled, normalized demangled, or short demangled names
/// (MSVC data symbols demangle to `struct Packet g_packet`; their short name is `g_packet`).
pub(crate) fn matches(
    entry: &SymbolEntry,
    filter: &SymbolFilter,
    demanglers: &[Arc<dyn Demangler>],
    languages: &[Arc<dyn LanguageSupport>],
) -> bool {
    if filter.kind.is_some_and(|k| k != entry.kind) {
        return false;
    }
    if filter.section.as_ref().is_some_and(|s| entry.section.as_ref().is_none_or(|k| &k.name != s)) {
        return false;
    }
    let Some(name) = &filter.name else { return true };
    let normalize = |n: &str| languages.first().map_or(n.to_owned(), |l| l.normalize_type_name(n));
    // Mach-O prefixes C symbols with `_`.
    entry.raw_name == *name
        || entry.raw_name.strip_prefix('_') == Some(name.as_str())
        || entry.demangled.as_deref() == Some(name.as_str())
        || entry.demangled.as_deref().is_some_and(|d| normalize(d) == normalize(name))
        || (entry.demangled.is_some()
            && demanglers
                .iter()
                .find(|d| d.recognizes(&entry.raw_name))
                .and_then(|d| d.demangle(&entry.raw_name, NameStyle::Short))
                .is_some_and(|short| short == *name || normalize(&short) == normalize(name)))
}

pub(crate) fn result(
    query: &str,
    table: &[SymbolEntry],
    filter: &SymbolFilter,
    demanglers: &[Arc<dyn Demangler>],
    languages: &[Arc<dyn LanguageSupport>],
) -> SymbolResult {
    SymbolResult {
        query: query.to_owned(),
        matches: table.iter().filter(|e| matches(e, filter, demanglers, languages)).cloned().collect(),
        diagnostics: Vec::new(),
    }
}
