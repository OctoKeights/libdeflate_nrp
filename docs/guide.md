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
| 0 | `Level::NONE` | Data stored as is, in blocks of up to 65,535 bytes |
| 2 to 4 | `Level::FASTEST` is 2 | Greedy: always take the longest match found |
| 5 to 7 | `Level::DEFAULT` is 6 | Lazy: defer a match if the next position has a better one |
| 8 and 9 | `Level::BEST` is 9 | Lazy, looking two positions ahead |

Higher levels also search the hash chains more deeply. At every level above 0, blocks end
where the data's statistics change, and each block is written in whichever of its three
forms (stored, fixed codes, or codes of its own) is smallest. Inputs of a few dozen bytes or
fewer are stored, as libdeflate stores them.

`Level::new` accepts 0 and 2 to 9 and returns `None` otherwise; `Level::try_from` does the
same with an error.

## Compressing

`zlib_compress` and `deflate_compress` take the input and a level and return the stream:

```rust
use libdeflate_nrp::{Level, deflate_compress, deflate_compress_bound};

let data = vec![7u8; 10_000];
let level = Level::new(9).expect("level 9 is supported");
let stream = deflate_compress(&data, level);
assert!(stream.len() <= deflate_compress_bound(data.len()));
```

`zlib_compress_bound` and `deflate_compress_bound` give the most bytes either can produce,
at any level, as libdeflate computes it.

## Decompressing

`zlib_decompress` and `deflate_decompress` decompress into a buffer the caller provides,
which must be large enough for the output. They return a `Decompressed`, giving the bytes
read and written, or an `Error`:

- `Error::BadData` for anything that is not a valid stream, including a truncated one and,
  for zlib, a wrong header or checksum.

- `Error::InsufficientSpace` when the output does not fit.

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
| `libdeflate_alloc_compressor(level)` with `libdeflate_zlib_compress` | `zlib_compress(input, level)` |
| `libdeflate_deflate_compress` | `deflate_compress` |
| `libdeflate_zlib_compress_bound`, `libdeflate_deflate_compress_bound` | `zlib_compress_bound`, `deflate_compress_bound` |
| `libdeflate_zlib_decompress_ex`, `libdeflate_deflate_decompress_ex` | `zlib_decompress`, `deflate_decompress` |
| `actual_in_nbytes_ret`, `actual_out_nbytes_ret` | `Decompressed::bytes_read`, `Decompressed::bytes_written` |
| `LIBDEFLATE_BAD_DATA`, `LIBDEFLATE_INSUFFICIENT_SPACE` | `Error::BadData`, `Error::InsufficientSpace` |
| `libdeflate_adler32` | `adler32` (from 1) and `adler32_update` |

## Differences from libdeflate

The code is written as idiomatic Rust rather than translated line by line; what it computes
is unchanged.

- **No compressor objects.** Compression is a function call, and its output is a `Vec`
  sized to fit, so there is no output buffer to run out of.

- **One decoding loop.** libdeflate decodes with a fast loop in the middle of its buffers
  and a careful one near their ends. This crate always uses the careful one, which produces
  the same output and fails for the same reasons. Like libdeflate, it treats up to eight
  bytes past the end of the input as zeros before declaring the stream truncated.

- **Not ported:** compression level 1, levels 10 to 12, gzip streams, CRC-32, and
  libdeflate's processor-specific code, which produces the same bytes.
