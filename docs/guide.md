# Guide

[Overview](../README.md) · **Guide** · [Verification](verification.md) · [Performance](performance.md)

---

- [Compression levels](#compression-levels)
- [Compressing](#compressing)
- [Decompressing](#decompressing)
- [Correspondence with libdeflate](#correspondence-with-libdeflate)
- [Differences from libdeflate](#differences-from-libdeflate)

## Compression levels

| Level | Constant | Strategy |
|---|---|---|
| 0 | `Level::NONE` / `Level::L0` | Data stored as is, in blocks of up to 65,535 bytes |
| 1 | `Level::FASTEST` / `Level::L1` | Fast two-way hash table for 4-byte matches |
| 2 to 4 | `Level::L2` .. `Level::L4` | Greedy hash chains: take the longest match found |
| 5 to 7 | `Level::DEFAULT` (`Level::L6`) | Lazy hash chains: defer a match if the next position has a better one |
| 8 and 9 | `Level::BEST` (`Level::L9`) | Lazy hash chains, looking two positions ahead |

Levels 0 and 2 to 9 produce bit-identical streams to libdeflate. At every level above 0,
blocks end where the data's statistics change, and each block is written in whichever of its
three forms (stored, fixed codes, or codes of its own) is smallest. Inputs of a few dozen
bytes or fewer are stored, as libdeflate stores them.

`Level::new` accepts 0 to 9 and returns `None` otherwise; `Level::try_from` does the same
with `UnsupportedLevel`.

## Compressing

`zlib_compress`, `deflate_compress`, and `gzip_compress` take the input and a level and
return a `Vec<u8>`. `zlib_compress_into`, `deflate_compress_into`, and `gzip_compress_into`
write directly into a caller-supplied slice, and `Compressor` reuses working buffers across
calls:

```rust
use libdeflate_nrp::{Compressor, Level, deflate_compress_bound};

let data = vec![7u8; 10_000];
let mut compressor = Compressor::new(Level::L9);
let mut buf = vec![0u8; deflate_compress_bound(data.len())];
let written = compressor.deflate_compress_into(&data, &mut buf).unwrap();
assert!(written <= buf.len());
```

`zlib_compress_bound`, `deflate_compress_bound`, and `gzip_compress_bound` are `const fn`
and give the most bytes each format can produce at any level.

## Decompressing

`zlib_decompress`, `deflate_decompress`, and `gzip_decompress` decompress into a buffer the
caller provides with zero heap allocations (or via a reusable `Decompressor`), returning a
`Decompressed` or an `Error`. `zlib_decompress_vec`, `deflate_decompress_vec`, and
`gzip_decompress_vec` allocate up to a caller-specified `max_len`:

- `Error::BadData` for anything that is not a valid stream, including a truncated one and,
  for zlib or gzip, a wrong header, checksum, or length trailer.

- `Error::InsufficientSpace` when the output does not fit in the buffer or exceeds `max_len`.

Input after the end of the stream is ignored; `bytes_read` says where the stream ended, so
several streams can be read one after another:

```rust
use libdeflate_nrp::{Level, zlib_compress, zlib_decompress};

let mut input = zlib_compress(b"first", Level::DEFAULT);
input.extend(zlib_compress(b"second", Level::DEFAULT));

let mut output = [0; 16];
let first = zlib_decompress(&input, &mut output).unwrap();
assert_eq!(&output[..first.bytes_written], b"first");
let second = zlib_decompress(&input[first.bytes_read..], &mut output).unwrap();
assert_eq!(&output[..second.bytes_written], b"second");
```

## Correspondence with libdeflate

| libdeflate | libdeflate_nrp |
|---|---|
| `libdeflate_alloc_compressor(level)` | `Compressor::new(level)` |
| `libdeflate_alloc_decompressor()` | `Decompressor::new()` |
| `libdeflate_deflate_compress`, `libdeflate_zlib_compress`, `libdeflate_gzip_compress` | `deflate_compress` / `*_into`, `zlib_compress` / `*_into`, `gzip_compress` / `*_into` |
| `libdeflate_*_compress_bound` | `deflate_compress_bound`, `zlib_compress_bound`, `gzip_compress_bound` |
| `libdeflate_*_decompress_ex` | `deflate_decompress`, `zlib_decompress`, `gzip_decompress` |
| `actual_in_nbytes_ret`, `actual_out_nbytes_ret` | `Decompressed::bytes_read`, `Decompressed::bytes_written` |
| `LIBDEFLATE_BAD_DATA`, `LIBDEFLATE_INSUFFICIENT_SPACE` | `Error::BadData`, `Error::InsufficientSpace` |
| `libdeflate_adler32`, `libdeflate_crc32` | `adler32` / `adler32_update`, `crc32` / `crc32_update` |

## Differences from libdeflate

The code is written as idiomatic, safe `#![no_std]` Rust (`core` + `alloc`) rather than
translated line by line; at levels 0 and 2 to 9, what it computes is byte-for-byte
identical.

- **Safe interior fast loop.** Away from buffer boundaries, the decompressor refills 64 bits
  at a time and decodes from compact two-level inline tables; near buffer boundaries, it
  falls back to byte-by-byte refilling so truncated and damaged streams fail identically to
  libdeflate.

- **Not ported:** near-optimal compression levels 10 to 12 and processor-specific SIMD/BMI2
  assembly routines.
