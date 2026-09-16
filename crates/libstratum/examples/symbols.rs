//! Lists symbols with sizes and size evidence: `cargo run -p libstratum --example symbols -- <binary> [name]`.
//! API documentation and a manual check, not a product CLI (ADR-0020).

use libstratum::model::{Evidence, SymbolFilter};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: symbols <binary> [name]");
        std::process::exit(2);
    };
    let engine = libstratum::default_engine();
    let session = match libstratum::open_path(&engine, &path) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("{path}: {err}");
            std::process::exit(1);
        }
    };
    let filter = SymbolFilter { name: args.next(), ..Default::default() };
    for s in session.symbols(&filter) {
        let size = s.size.map_or("-".into(), |n| n.to_string());
        let evidence = match &s.size_evidence {
            Some(Evidence::SymbolTable) => "symtab",
            Some(Evidence::DebugInfo { .. }) => "debuginfo",
            Some(Evidence::LinkMap { .. }) => "map",
            Some(Evidence::Heuristic { .. }) => "heuristic",
            _ => "-",
        };
        let section = s.section.as_ref().map_or("-".into(), |k| k.name.clone());
        let fold = if s.folded { " FOLDED" } else { "" };
        let aliases = if s.aliases.is_empty() { String::new() } else { format!(" aliases={:?}", s.aliases) };
        println!(
            "{:#014x} {:>6} {:<9} {:<9?} {:<12} {}{}{}",
            s.address,
            size,
            evidence,
            s.kind,
            section,
            s.demangled.as_deref().unwrap_or(&s.raw_name),
            fold,
            aliases
        );
    }
}
