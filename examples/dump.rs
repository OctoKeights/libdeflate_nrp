//! The Rust side of the comparison with libdeflate: runs `libdeflate_nrp` over synthetic data
//! and prints a running hash of its output, as `compare/dump_libdeflate.c` does with
//! libdeflate. `compare/compare.sh` compares the two.
//!
//!   dump libdeflate <seed> <count>   synthetic data of eight kinds and many sizes, sometimes
//!                                    several pieces joined, compressed to a zlib stream at a
//!                                    level; each stream decompressed intact, into too little
//!                                    room and into more, damaged, truncated and with bytes
//!                                    after it; and random bytes after a zlib header

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
    if what.as_str() == "libdeflate" {
        print!("{}", made_up::libdeflate(seed, count));
    } else {
        eprintln!("unknown dump {what}");
        std::process::exit(2);
    }
}
