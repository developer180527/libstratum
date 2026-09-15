//! Per-module, per-block line dump from both crates, to find the arm64 discrepancy.
use pdb2::FallibleIterator;
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap();
    // ms-pdb: module name -> Vec<(seg, offset, line_count_in_block)> plus raw line records
    let pdb = ms_pdb::Pdb::open(path.as_ref())?;
    let mut ms: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    for m in pdb.modules()?.iter() {
        let Some(modi) = pdb.read_module_stream(&m)? else { continue };
        let e = ms.entry(m.module_name().to_string()).or_default();
        for sub in modi.c13_line_data().subsections() {
            if sub.kind == ms_pdb::lines::SubsectionKind::LINES {
                let ls = ms_pdb::lines::LinesSubsection::parse(sub.data)?;
                for b in ls.blocks() {
                    for l in b.lines() {
                        e.push((l.offset.get(), l.line_num_start()));
                    }
                }
            }
        }
    }
    let mut p2 = pdb2::PDB::open(std::fs::File::open(&path)?)?;
    let dbi = p2.debug_information()?;
    let mut mods = dbi.modules()?;
    let mut pd: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    while let Some(m) = mods.next()? {
        let name = m.module_name().to_string();
        let Some(info) = p2.module_info(&m)? else { continue };
        let prog = info.line_program()?;
        let mut it = prog.lines();
        let e = pd.entry(name).or_default();
        while let Some(l) = it.next()? {
            e.push((l.offset.offset, l.line_start));
        }
    }
    for (name, a) in &ms {
        let b = pd.get(name).cloned().unwrap_or_default();
        if a.len() != b.len() {
            let mut sa = a.clone(); sa.sort();
            let mut sb = b.clone(); sb.sort();
            let only_ms: Vec<_> = sa.iter().filter(|x| !sb.contains(x)).collect();
            let only_p2: Vec<_> = sb.iter().filter(|x| !sa.contains(x)).collect();
            println!("{name}: ms-pdb {} vs pdb2 {}\n  only ms-pdb (offset,line): {:?}\n  only pdb2: {:?}", a.len(), b.len(), only_ms, only_p2);
        }
    }
    Ok(())
}
