//! Q14 spike: compare ms-pdb and pdb2 on the fixture PDBs.
//! Output per PDB: a canonical layout dump (sorted) and structural counts, per crate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

#[derive(Default, Debug, PartialEq)]
struct Report {
    layouts: BTreeSet<String>,
    modules: usize,
    procs: usize,
    inline_sites: usize,
    line_records: usize,
    publics: usize,
    public_rva_groups: BTreeSet<String>, // names sharing one address (ICF)
}

const FIXTURE_TYPES: &[&str] = &[
    "Packet", "PacketReordered", "CacheStraddle", "Empty", "Flags", "RegisterBits", "WithEmptyBase", "Multiple",
    "Polymorphic", "Derived", "Diamond", "VLeft", "Variant", "engine::Outer", "engine::Outer::Inner", "Aligned",
    "Holder", "WireHeader", "Pack2", "Pair<char,double>", "Pair<double,char>", "SmallArray<short,3>", "Point",
    "Number", "Message", "Config",
];

// ----------------------------------------------------------------------------- ms-pdb

mod mspdb {
    use super::*;
    use ms_pdb::codeview::syms::SymData;
    use ms_pdb::types::fields::Field;
    use ms_pdb::types::{Leaf, TypeData, TypeIndex};

    pub fn run(path: &Path) -> anyhow::Result<Report> {
        let pdb = ms_pdb::Pdb::open(path)?;
        let mut r = Report::default();
        let tpi = pdb.read_type_stream()?;
        let begin = tpi.type_index_begin();

        for (i, rec) in tpi.iter_type_records().enumerate() {
            let (kind, name, size, fwd, fields) = match rec.parse() {
                Ok(TypeData::Struct(s)) => {
                    let p = s.fixed.property.get();
                    ("struct", s.name.to_string(), u64::try_from(s.length).unwrap_or(u64::MAX), p.fwdref(), s.fixed.field_list.get())
                }
                Ok(TypeData::Union(u)) => {
                    let p = u.fixed.property.get();
                    ("union", u.name.to_string(), u64::try_from(u.length).unwrap_or(u64::MAX), p.fwdref(), u.fixed.fields.get())
                }
                _ => continue,
            };
            let _ = i;
            if fwd || !FIXTURE_TYPES.contains(&name.as_str()) {
                continue;
            }
            let mut members = Vec::new();
            let mut fl = Some(fields);
            while let Some(ti) = fl.take() {
                let TypeData::FieldList(list) = tpi.record(ti)?.parse()? else { break };
                for f in list.iter() {
                    match f {
                        Field::Member(m) => {
                            let off = u64::try_from(m.offset).unwrap_or(u64::MAX);
                            let mut desc = format!("M {} @{}", m.name, off);
                            if m.ty.0 >= begin.0 {
                                let rec = tpi.record(m.ty)?;
                                if rec.kind == Leaf::LF_BITFIELD {
                                    if let Ok(TypeData::Bitfield(b)) = rec.parse() {
                                        desc = format!("{desc} bit{}:{}", b.position, b.length);
                                    }
                                }
                            }
                            members.push(desc);
                        }
                        Field::BaseClass(b) => members.push(format!("B @{}", u64::try_from(b.offset).unwrap_or(u64::MAX))),
                        Field::DirectVirtualBaseClass(_) | Field::IndirectVirtualBaseClass(_) => members.push("VB".into()),
                        Field::VFuncTable(_) => members.push("VFPTR".into()),
                        Field::Index(next) => fl = Some(next),
                        _ => {}
                    }
                }
            }
            r.layouts.insert(format!("{kind} {name} size={size} [{}]", members.join(", ")));
        }

        for module in pdb.modules()?.iter() {
            r.modules += 1;
            let Some(modi) = pdb.read_module_stream(&module)? else { continue };
            for sym in modi.iter_syms() {
                match sym.parse() {
                    Ok(SymData::Proc(_)) => r.procs += 1,
                    Ok(SymData::InlineSite(_)) | Ok(SymData::InlineSite2(_)) => r.inline_sites += 1,
                    _ => {}
                }
            }
            for sub in modi.c13_line_data().subsections() {
                if sub.kind == ms_pdb::lines::SubsectionKind::LINES {
                    if let Ok(lines) = ms_pdb::lines::LinesSubsection::parse(sub.data) {
                        for block in lines.blocks() {
                            r.line_records += block.lines().len();
                        }
                    }
                }
            }
        }

        let mut by_addr: BTreeMap<(u16, u32), Vec<String>> = BTreeMap::new();
        for sym in pdb.gss()?.iter_syms() {
            if let Ok(SymData::Pub(p)) = sym.parse() {
                r.publics += 1;
                let os = p.offset_segment();
                by_addr.entry((os.segment(), os.offset())).or_default().push(p.name.to_string());
            }
        }
        r.public_rva_groups = groups(by_addr);
        let _ = TypeIndex(0);
        Ok(r)
    }
}

// ----------------------------------------------------------------------------- pdb2

mod pdb2x {
    use super::*;
    use pdb2::{FallibleIterator, SymbolData, TypeData};

    pub fn run(path: &Path) -> anyhow::Result<Report> {
        let mut pdb = pdb2::PDB::open(std::fs::File::open(path)?)?;
        let mut r = Report::default();
        let ti = pdb.type_information()?;
        let mut finder = ti.finder();
        let mut iter = ti.iter();
        let mut candidates = Vec::new();
        while let Some(t) = iter.next()? {
            finder.update(&iter);
            match t.parse() {
                Ok(TypeData::Class(c)) if !c.properties.forward_reference() => {
                    if let Some(f) = c.fields {
                        candidates.push(("struct", c.name.to_string().into_owned(), c.size, f));
                    }
                }
                Ok(TypeData::Union(u)) if !u.properties.forward_reference() => {
                    candidates.push(("union", u.name.to_string().into_owned(), u.size, u.fields));
                }
                _ => {}
            }
        }
        for (kind, name, size, fields) in candidates {
            if !FIXTURE_TYPES.contains(&name.as_str()) {
                continue;
            }
            let mut members = Vec::new();
            let mut fl = Some(fields);
            while let Some(idx) = fl.take() {
                let TypeData::FieldList(list) = finder.find(idx)?.parse()? else { break };
                for f in list.fields {
                    match f {
                        TypeData::Member(m) => {
                            let mut desc = format!("M {} @{}", m.name, m.offset);
                            if let Ok(TypeData::Bitfield(b)) = finder.find(m.field_type).and_then(|t| t.parse()) {
                                desc = format!("{desc} bit{}:{}", b.position, b.length);
                            }
                            members.push(desc);
                        }
                        TypeData::BaseClass(b) => members.push(format!("B @{}", b.offset)),
                        TypeData::VirtualBaseClass(_) => members.push("VB".into()),
                        TypeData::VirtualFunctionTablePointer(_) => members.push("VFPTR".into()),
                        _ => {}
                    }
                }
                fl = list.continuation;
            }
            r.layouts.insert(format!("{kind} {name} size={size} [{}]", members.join(", ")));
        }

        let dbi = pdb.debug_information()?;
        let mut modules = dbi.modules()?;
        while let Some(module) = modules.next()? {
            r.modules += 1;
            let Some(info) = pdb.module_info(&module)? else { continue };
            let mut syms = info.symbols()?;
            while let Some(s) = syms.next()? {
                match s.parse() {
                    Ok(SymbolData::Procedure(_)) => r.procs += 1,
                    Ok(SymbolData::InlineSite(_)) => r.inline_sites += 1,
                    _ => {}
                }
            }
            let program = info.line_program()?;
            let mut lines = program.lines();
            while lines.next()?.is_some() {
                r.line_records += 1;
            }
        }

        let mut by_addr: BTreeMap<(u16, u32), Vec<String>> = BTreeMap::new();
        let globals = pdb.global_symbols()?;
        let mut it = globals.iter();
        while let Some(s) = it.next()? {
            if let Ok(SymbolData::Public(p)) = s.parse() {
                r.publics += 1;
                by_addr.entry((p.offset.section, p.offset.offset)).or_default().push(p.name.to_string().into_owned());
            }
        }
        r.public_rva_groups = groups(by_addr);
        Ok(r)
    }
}

fn groups(by_addr: BTreeMap<(u16, u32), Vec<String>>) -> BTreeSet<String> {
    by_addr
        .into_values()
        .filter(|v| v.len() > 1)
        .map(|mut v| {
            v.sort();
            v.join(" = ")
        })
        .collect()
}

fn main() -> anyhow::Result<()> {
    let root = std::env::args().nth(1).expect("fixtures/bin path");
    let verbose = std::env::args().any(|a| a == "-v");
    let mut pdbs: Vec<_> = walk(Path::new(&root)).into_iter().filter(|p| p.extension().is_some_and(|e| e == "pdb") && !p.to_string_lossy().ends_with(".compile.pdb")).collect();
    pdbs.sort();
    let (mut agree, mut t_ms, mut t_p2) = (0, 0f64, 0f64);
    for p in &pdbs {
        let s = Instant::now();
        let a = mspdb::run(p);
        t_ms += s.elapsed().as_secs_f64();
        let s = Instant::now();
        let b = pdb2x::run(p);
        t_p2 += s.elapsed().as_secs_f64();
        let rel = p.strip_prefix(&root).unwrap().display().to_string();
        match (&a, &b) {
            (Ok(a), Ok(b)) => {
                let same = a == b;
                agree += same as usize;
                println!(
                    "{} {rel}: types={} modules={} procs={} inline={} lines={}/{} publics={} icf_groups={}",
                    if same { "SAME" } else { "DIFF" },
                    a.layouts.len(), a.modules, a.procs, a.inline_sites, a.line_records, b.line_records, a.publics, a.public_rva_groups.len()
                );
                if !same || verbose {
                    for l in a.layouts.symmetric_difference(&b.layouts) {
                        println!("    {} {l}", if a.layouts.contains(l) { "ms-pdb only:" } else { "pdb2 only:  " });
                    }
                    if a.modules != b.modules || a.procs != b.procs || a.inline_sites != b.inline_sites || a.publics != b.publics || a.public_rva_groups != b.public_rva_groups {
                        println!("    counts ms-pdb {:?}\n    counts pdb2   {:?}", (a.modules, a.procs, a.inline_sites, a.publics, a.public_rva_groups.len()), (b.modules, b.procs, b.inline_sites, b.publics, b.public_rva_groups.len()));
                    }
                }
                if verbose {
                    for l in &a.layouts { println!("    {l}"); }
                    for g in &a.public_rva_groups { println!("    ICF: {g}"); }
                }
            }
            _ => println!("ERR  {rel}: ms-pdb={:?} pdb2={:?}", a.as_ref().err(), b.as_ref().err()),
        }
    }
    println!("\n{agree}/{} PDBs identical. time ms-pdb {:.2}s, pdb2 {:.2}s", pdbs.len(), t_ms, t_p2);
    Ok(())
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { out.extend(walk(&p)); } else { out.push(p); }
        }
    }
    out
}
