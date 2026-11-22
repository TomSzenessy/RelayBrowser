//! Contract check used by js/test: reads one bridge message per stdin line
//! and fails on the first one Rust cannot parse.

use std::io::BufRead;

fn main() {
    let mut n = 0;
    for line in std::io::stdin().lock().lines() {
        let line = line.expect("stdin");
        if line.trim().is_empty() {
            continue;
        }
        if let Err(e) = relay_bridge::PageMessage::parse(&line) {
            eprintln!("unparseable bridge message: {e}\n{line}");
            std::process::exit(1);
        }
        n += 1;
    }
    println!("{n} messages parsed");
}
