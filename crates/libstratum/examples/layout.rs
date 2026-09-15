//! Prints the memory layout of a type: `cargo run -p libstratum --example layout -- <binary> <TypeName>`.
//! API documentation and a manual check, not a product CLI (ADR-0020).

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(name)) = (args.next(), args.next()) else {
        eprintln!("usage: layout <binary> <TypeName>");
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
    match session.struct_layout(&name) {
        Ok(result) => {
            for layout in &result.matches {
                println!(
                    "{} {:?}: {} bytes, align {:?}, padding {} bits, packed {}",
                    layout.name,
                    layout.kind,
                    layout.size_bytes,
                    layout.alignment_bytes,
                    layout.padding_bits,
                    layout.packed
                );
                for m in &layout.members {
                    let off = m.offset_bits.map_or("?".into(), |o| {
                        if o % 8 == 0 { format!("{}", o / 8) } else { format!("{}.{}", o / 8, o % 8) }
                    });
                    println!(
                        "  @{off:<6} {:>4}b {:?} {} : {}",
                        m.size_bits,
                        m.kind,
                        m.name.as_deref().unwrap_or("-"),
                        m.type_name.as_deref().unwrap_or("-")
                    );
                }
                for h in &layout.holes {
                    println!("  hole @{} bits, {} bits", h.offset_bits, h.size_bits);
                }
                if let Some(s) = &layout.suggestion {
                    println!("  suggestion: {:?} -> {} bytes (saves {})", s.order, s.new_size_bytes, s.saved_bytes);
                }
            }
            if let Some(reason) = &result.not_found_reason {
                println!("not found: {reason}");
            }
            for d in result.diagnostics.iter().chain(session.diagnostics()) {
                println!("diagnostic: {:?} {:?} {}", d.severity, d.code, d.message);
            }
        }
        Err(err) => eprintln!("{err}"),
    }
}
