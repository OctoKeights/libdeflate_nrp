# libdeflate_nrp

**DEFLATE and zlib in pure, safe Rust: a native Rust port of libdeflate.**

libdeflate_nrp is a line-for-line port of Eric Biggers's
[libdeflate](https://github.com/ebiggers/libdeflate) v1.26: its compressor, which gives the
streams libdeflate gives, byte for byte, at its levels 0 and 2 to 9; and its decompressor,
which takes and refuses the streams libdeflate takes and refuses.

## Highlights

- **Bit-identical streams.** The greedy, lazy and lazy2 parsers over hash chains, the
  splitting of blocks and the choice of the cheapest kind of block, as libdeflate has them.
  This is what a format that stores libdeflate's output needs (OpenEXR's DWAA and DWAB, as
  [openexr_nrp](https://github.com/OctoKeights/openexr_nrp) writes them, for one).

- **Every stream read.** Raw DEFLATE and zlib, with the bytes taken and made, as libdeflate's
  `_ex` functions answer.

- **Zero dependencies.** Standard library only: no C compiler, no build script.

- **No `unsafe` code.** Enforced by `#![forbid(unsafe_code)]`.

## Installation

```toml
[dependencies]
libdeflate_nrp = "0.1"
```

## Quick start

```rust
let data: Vec<u8> = b"to be or not to be, that is the question; ".repeat(100);

// A zlib stream at level 6, libdeflate's default (0 and 2 to 9 are ported).
let stream = libdeflate_nrp::zlib_compress(6, &data);
assert!(stream.len() <= libdeflate_nrp::zlib_compress_bound(data.len()));

// Decompressing answers the bytes it took of the input and the bytes it made.
let mut back = vec![0u8; data.len()];
assert_eq!(
    libdeflate_nrp::zlib_decompress(&stream, &mut back),
    Ok((stream.len(), data.len()))
);
assert_eq!(back, data);

// Too little room is refused.
let mut short = vec![0u8; data.len() - 1];
assert_eq!(
    libdeflate_nrp::zlib_decompress(&stream, &mut short),
    Err(libdeflate_nrp::Error::InsufficientSpace)
);
```

## What is ported

| Original | What it does |
|---|---|
| `deflate_compress.c`, `hc_matchfinder.h`, `matchfinder_common.h` | Bytes to a raw DEFLATE stream (`deflate_compress`, `deflate_compress_bound`) at levels 0 and 2 to 9 |
| `zlib_compress.c` | The same in a zlib stream (`zlib_compress`, `zlib_compress_bound`) |
| `deflate_decompress.c`, `decompress_template.h` | A raw DEFLATE stream to bytes (`deflate_decompress`) |
| `zlib_decompress.c` | A zlib stream to bytes (`zlib_decompress`) |
| `adler32.c` | The Adler-32 checksum (`adler32`) |

Not ported: level 1, which has a hash table of its own, and the near-optimal levels 10 to 12
(they panic); gzip streams and CRC-32; and the original's choice of code for each processor,
which makes the same bytes either way.

## Differences from the original

The code is translated from C to Rust. The compressor and decompressor are functions rather
than objects to allocate: the compressor's work areas are made for each call, and its output
is a `Vec` as long as it needs to be, so there is no output buffer that is too small. The
decompressor decodes as the original's careful loop does, codeword by codeword, rather than
with its fast loop for the middle of a buffer; both make the same bytes and stop for the same
reasons, and the port takes what the original reads past the end of its input as zeros, as it
does. What they compute is kept: the same stream, byte for byte, as libdeflate gives for a
source and a level; the same bytes from every stream; and the same refusal of a damaged one.

## Verification

`compare/compare.sh` builds libdeflate's library from a checkout, `compare/dump_libdeflate.c`
against it, and `examples/dump.rs` against the port; it runs both over the same made-up data
and compares what they print byte for byte:

```sh
compare/compare.sh <checkout of libdeflate> [count] [seeds] [dumps]
```

| Dump | What it covers | Last full run |
|---|---|---|
| `libdeflate` | Made-up data of eight kinds and many sizes, now and then several pieces together, compressed into zlib streams at levels 0 and 2 to 9; each stream decompressed whole, into too little and into more room, damaged, cut short and with bytes after it; and made-up bytes after a zlib header | 40,000 sources (1.2 gigabytes), every stream identical, and every decompression of it, whole and damaged, with the same bytes or the same refusal |

`cargo test` needs no C compiler: the tests check the port against hashes recorded from
libdeflate (`tests/data`).

## Licence

As libdeflate is: MIT ([LICENSE](LICENSE), which keeps libdeflate's copyright notices).
