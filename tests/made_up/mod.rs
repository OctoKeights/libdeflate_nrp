//! Synthetic data, generated from a seed exactly as the C side of the comparison
//! (`compare/dump_libdeflate.c`) generates it, and the dump that both the golden tests and
//! `examples/dump.rs` run.

// (The arithmetic is written as the C side has it; not every user of this module uses all of
// it.)
#![allow(
    clippy::manual_is_multiple_of,
    clippy::needless_range_loop,
    clippy::manual_div_ceil,
    clippy::manual_checked_ops,
    dead_code
)]

use std::fmt::Write;
/// The two programs make the same blocks from the same seed: a 64-bit linear congruential
/// generator, the high half of its state.
pub struct Random {
    pub state: u64,
}

impl Random {
    pub fn next(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state >> 32) as u32
    }
}

/// FNV-1a, 64 bits.
pub struct Hash {
    pub value: u64,
}

impl Hash {
    pub fn new() -> Hash {
        Hash {
            value: 14_695_981_039_346_656_037,
        }
    }

    pub fn add(&mut self, data: &[u8]) {
        for &byte in data {
            self.value ^= u64::from(byte);
            self.value = self.value.wrapping_mul(1_099_511_628_211);
        }
    }
}

const WORDS: [&str; 16] = [
    "the", "texture", "block", "of", "and", "compress", "a", "level", "mip", "to", "pixels", "in",
    "format", "is", "cube", "data",
];

/// Made-up data of one of eight kinds, the same in both programs.
pub fn make_data(r: &mut Random) -> Vec<u8> {
    let len = match r.next() % 8 {
        0 => r.next() % 16,
        1 => r.next() % 300,
        2 | 3 => r.next() % 5000,
        4 | 5 => r.next() % 40000,
        6 => 60000 + r.next() % 20000,
        _ => r.next() % 200_000,
    } as usize;
    let kind = r.next() % 8;
    let mut buf = Vec::with_capacity(len);
    match kind {
        // noise
        0 => {
            for _ in 0..len {
                buf.push((r.next() >> 13) as u8);
            }
        }
        // four letters
        1 => {
            for _ in 0..len {
                buf.push(b'a' + (r.next() % 4) as u8);
            }
        }
        // words
        2 => {
            while buf.len() < len {
                let word = WORDS[(r.next() % 16) as usize];
                for &byte in word.as_bytes() {
                    if buf.len() < len {
                        buf.push(byte);
                    }
                }
                if buf.len() < len {
                    buf.push(b' ');
                }
            }
        }
        // runs
        3 => {
            while buf.len() < len {
                let byte = r.next() as u8;
                let run = 1 + r.next() % 300;
                for _ in 0..run {
                    if buf.len() < len {
                        buf.push(byte);
                    }
                }
            }
        }
        // copies of what came before
        4 => {
            while buf.len() < len {
                if buf.len() < 64 || r.next() % 4 == 0 {
                    buf.push((r.next() >> 9) as u8);
                } else {
                    let reach = buf.len().min(40000);
                    let dist = 1 + r.next() as usize % reach;
                    let n = 3 + r.next() % 260;
                    for _ in 0..n {
                        if buf.len() < len {
                            buf.push(buf[buf.len() - dist]);
                        }
                    }
                }
            }
        }
        // nothing
        5 => buf.resize(len, 0),
        // a slope
        6 => {
            for i in 0..len {
                buf.push((i / 7 + i % 13) as u8);
            }
        }
        // stretches of noise, of one value and of counting
        _ => {
            while buf.len() < len {
                let (stretch, what, value) = (1 + r.next() % 2000, r.next() % 3, r.next());
                for i in 0..stretch {
                    if buf.len() < len {
                        buf.push(match what {
                            0 => (r.next() >> 11) as u8,
                            1 => value as u8,
                            _ => value.wrapping_add(i) as u8,
                        });
                    }
                }
            }
        }
    }
    buf
}

/// The `libdeflate` dump: synthetic data compressed into zlib streams at a level of
/// libdeflate's, and the streams (and damaged copies, and random bytes after a zlib header)
/// decompressed, as `compare/dump_libdeflate.c` does with libdeflate itself.
pub fn libdeflate(seed: u64, count: u32) -> String {
    use libdeflate_nrp::{Decompressed, Error, Level, zlib_compress, zlib_decompress};
    const LEVELS: [u8; 10] = [0, 2, 3, 4, 5, 6, 7, 8, 9, 4];
    fn add_uncompress(hash: &mut Hash, input: &[u8], room: usize) {
        let mut out = vec![0u8; room];
        // 0 for done, when the stream ends where the input does, and the bytes made; 1 for a
        // stream that is not one or does not end there; 2 for one that makes too many bytes.
        match zlib_decompress(input, &mut out) {
            Ok(Decompressed {
                bytes_read,
                bytes_written: made,
            }) if bytes_read == input.len() => {
                hash.add(&0u32.to_le_bytes());
                hash.add(&(made as u32).to_le_bytes());
                hash.add(&out[..made]);
            }
            Ok(_) | Err(Error::BadData) => hash.add(&1u32.to_le_bytes()),
            Err(Error::InsufficientSpace) => hash.add(&2u32.to_le_bytes()),
        }
    }
    let mut r = Random { state: seed };
    let mut hash = Hash::new();
    let mut total = 0u64;
    let mut out = String::new();
    let add_number = |hash: &mut Hash, value: u32| hash.add(&value.to_le_bytes());
    for round in 0..count {
        let mut data = make_data(&mut r);
        if r.next().is_multiple_of(8) {
            let more = 1 + r.next() % 2;
            for _ in 0..more {
                data.extend(make_data(&mut r));
            }
        }
        let level = LEVELS[(r.next() % 10) as usize];
        let mut stream = zlib_compress(&data, Level::new(level).expect("a ported level"));
        let size = stream.len();
        total += data.len() as u64;
        add_number(&mut hash, data.len() as u32);
        add_number(&mut hash, u32::from(level));
        add_number(&mut hash, size as u32);
        hash.add(&stream);

        let (how, pick, pick2) = (r.next() % 8, r.next(), r.next());
        let len = data.len();
        let mut room = len;
        let mut n = size;
        if how == 1 && len > 0 {
            room = len - 1 - (pick as usize) % len.min(16);
        } else if how == 2 {
            room = len + (pick % 100) as usize;
        } else if how == 3 {
            let flips = 1 + pick2 % 3;
            for _ in 0..flips {
                let bit = r.next() % (size as u32 * 8);
                stream[(bit >> 3) as usize] ^= 1 << (bit & 7);
            }
        } else if how == 4 {
            n = (pick as usize) % (size + 1);
        } else if how == 5 {
            let extra = 1 + pick2 % 9;
            for _ in 0..extra {
                stream.push(r.next() as u8);
            }
            n = size + extra as usize;
        } else if how == 6 {
            stream[(pick as usize) % size] = pick2 as u8;
        }
        add_number(&mut hash, how);
        add_uncompress(&mut hash, &stream[..n], room);

        if round % 4 == 0 {
            let n = 6 + (r.next() % 400) as usize;
            let mut bytes = vec![0x78u8, 0x5e];
            for _ in 2..n {
                bytes.push((r.next() >> 7) as u8);
            }
            if r.next() % 2 != 0 {
                bytes[2] = (bytes[2] & !7) | 1 | (((r.next() % 3) as u8) << 1);
            }
            let room = (r.next() % 3000) as usize;
            add_uncompress(&mut hash, &bytes, room);
        }

        if (round + 1) % 200 == 0 || round + 1 == count {
            writeln!(out, "libdeflate {} {:016x}", round + 1, hash.value).unwrap();
        }
    }
    out + &format!("libdeflate bytes: {total}\n")
}
