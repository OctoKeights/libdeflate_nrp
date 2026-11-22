# libdeflate_nrp

**DEFLATE, zlib, and gzip compression in pure, safe `#![no_std]` Rust, bit-identical to libdeflate.**

**Overview** · [Guide](docs/guide.md) · [Verification](docs/verification.md) · [Performance](docs/performance.md)

---

libdeflate_nrp is a native Rust port of Eric Biggers's
[libdeflate](https://github.com/ebiggers/libdeflate) v1.26. Its compressor produces the
same streams as libdeflate, byte for byte, at levels 0 and 2 to 9 (plus a fast hash-table
level 1), and its decompressor accepts and rejects the same streams libdeflate does.

## Highlights

- **Bit-identical output.** Every stream at levels 0 and 2 to 9 matches libdeflate's at the
  same level, which matters for formats that store libdeflate's output, such as OpenEXR's
  DWA compression.

- **Exact, zero-allocation decompression.** Valid streams decompress to the same bytes, and
  damaged ones fail with the same error, using compact two-level inline tables with zero
  heap allocations.

- **Zero dependencies & `#![no_std]`.** Uses `core` and `alloc` only: no C compiler, no
  build script.

- **No `unsafe` code.** Enforced by `#![forbid(unsafe_code)]`.

## Installation

```toml
[dependencies]
libdeflate_nrp = "0.2"
```

## Quick start

```rust
use libdeflate_nrp::{Error, Level, zlib_compress, zlib_compress_bound, zlib_decompress};

let data = b"to be or not to be, that is the question; ".repeat(100);

let stream = zlib_compress(&data, Level::DEFAULT);
assert!(stream.len() <= zlib_compress_bound(data.len()));

let mut decompressed = vec![0; data.len()];
let result = zlib_decompress(&stream, &mut decompressed).unwrap();
assert_eq!(result.bytes_written, data.len());
assert_eq!(decompressed, data);

// An output buffer that is too small is reported, not overrun.
let mut short = vec![0; data.len() - 1];
assert_eq!(zlib_decompress(&stream, &mut short), Err(Error::InsufficientSpace));
```

## Scope

Raw DEFLATE, zlib, and RFC 1952 gzip streams in both directions, with Adler-32 and CRC-32
checksums, reusable `Compressor` and `Decompressor` types, slice-based `*_compress_into`,
and bounded `*_decompress_vec` helpers. Compression levels 0 to 9 are supported; levels 10
to 12 are not ported.

For general-purpose zlib compression where libdeflate's exact output does not matter,
[miniz_nrp](https://github.com/OctoKeights/miniz_nrp) is an alternative.

## Licence

MIT, as libdeflate is; see [`LICENSE`](LICENSE), which keeps libdeflate's copyright notices.

Bug reports and contributions are welcome, above all any stream that libdeflate_nrp
compresses or decompresses differently from libdeflate.
