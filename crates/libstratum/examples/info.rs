//! Prints what libstratum currently knows about a binary.
//! `cargo run -p libstratum --example info -- path/to/binary`
//! API documentation and a manual check, not a product CLI (ADR-0020).

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: info <binary>");
        std::process::exit(2);
    };
    let engine = libstratum::default_engine();
    match libstratum::open_path(&engine, &path) {
        Ok(session) => {
            println!("{:#?}", session.info());
            println!("{:#?}", session.capabilities());
        }
        Err(err) => {
            eprintln!("{path}: {err}");
            std::process::exit(1);
        }
    }
}
