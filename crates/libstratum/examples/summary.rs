//! Where the bytes go: `cargo run -p libstratum --example summary -- <binary> [--symbols] [--top N]`.
//! API documentation and a manual check, not a product CLI (ADR-0020).

use libstratum::model::{Dimension, Evidence, SummaryRow};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.iter().find(|a| {
        !a.starts_with("--") && args.iter().position(|b| b == "--top").is_none_or(|p| args.get(p + 1) != Some(a))
    }) else {
        eprintln!("usage: summary <binary> [--symbols] [--top N]");
        std::process::exit(2);
    };
    let top: usize =
        args.iter().position(|a| a == "--top").and_then(|p| args.get(p + 1)).and_then(|n| n.parse().ok()).unwrap_or(25);
    let dimensions = if args.iter().any(|a| a == "--symbols") {
        vec![Dimension::Section, Dimension::Symbol]
    } else {
        vec![Dimension::Section]
    };
    let engine = libstratum::default_engine();
    let session = match libstratum::open_path(&engine, path) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("{path}: {err}");
            std::process::exit(1);
        }
    };
    let summary = match session.summary(&dimensions) {
        Ok(summary) => summary,
        Err(err) => {
            eprintln!("{path}: {err}");
            std::process::exit(1);
        }
    };
    // `~` marks rows whose size rests on a heuristic (next-symbol distance).
    let print = |row: &SummaryRow| {
        let heuristic = row.evidence.iter().any(|e| matches!(e, Evidence::Heuristic { .. }));
        let mark = if heuristic { "~" } else { " " };
        println!("{:>12} {:>12} {:>12} {mark}{}", row.size.vm, row.size.file, row.size.load, row.path.join("  "));
    };
    println!("{:>12} {:>12} {:>12}  path", "vm", "file", "load");
    let mut rows: Vec<&SummaryRow> = summary.rows.iter().chain(&summary.unattributed).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.size.vm.max(r.size.file)));
    for row in rows.iter().take(top) {
        print(row);
    }
    if rows.len() > top {
        println!("{:>40}  ({} more rows)", "…", rows.len() - top);
    }
    println!(
        "{:>12} {:>12} {:>12}  TOTAL (shared: vm {} file {})",
        summary.total.vm, summary.total.file, summary.total.load, summary.shared.vm, summary.shared.file
    );
    let b = session.berkeley_sizes();
    println!("berkeley: text {} data {} bss {} dec {}", b.text, b.data, b.bss, b.text + b.data + b.bss);
    for d in &summary.diagnostics {
        println!("{:?} {:?}: {}", d.severity, d.code, d.message);
    }
}
