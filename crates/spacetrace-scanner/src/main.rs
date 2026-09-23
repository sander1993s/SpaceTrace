use spacetrace_scanner::scan;
use std::{
    io::{self, Write},
    path::PathBuf,
    sync::atomic::AtomicBool,
};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(root) = args.next() else {
        eprintln!("Usage: spacetrace-scan <folder>\nWrites read-only scan events as newline-delimited JSON to stdout.");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("Expected exactly one folder argument.");
        std::process::exit(2);
    }
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let result = scan(&PathBuf::from(root), &AtomicBool::new(false), |event| {
        serde_json::to_writer(&mut out, &event).map_err(|e| e.to_string())?;
        out.write_all(b"\n").map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())
    });
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
