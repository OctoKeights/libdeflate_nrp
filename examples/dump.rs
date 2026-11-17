//! The Rust half of the comparison with libdeflate: runs the port over made-up data and
//! prints a running hash of what comes out, as `compare/dump_libdeflate.c` does with the original.
//! `compare/compare.sh` compares the two.
//!
//!   dump libdeflate <seed> <count>   made-up data of eight kinds and many sizes, now and then
//!                                    several pieces together, compressed into a zlib stream at
//!                                    a level; each stream decompressed whole, into too little
//!                                    room and into more, damaged, cut short and with bytes after
//!                                    it; and made-up bytes after a zlib header decompressed

#[path = "../tests/made_up/mod.rs"]
mod made_up;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(what), Some(seed), Some(count)) = (
        args.get(1),
        args.get(2).and_then(|s| s.parse::<u64>().ok()),
        args.get(3).and_then(|s| s.parse::<u32>().ok()),
    ) else {
        eprintln!("usage: dump <what> <seed> <count>   (see the top of dump.rs)");
        std::process::exit(2);
    };
    let _ = seed;
    match what.as_str() {
        "libdeflate" => print!("{}", made_up::libdeflate(seed, count)),
        _ => {
            eprintln!("unknown dump {what}");
            std::process::exit(2);
        }
    }
}
