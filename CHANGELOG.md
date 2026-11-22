# Changelog

## 0.3.0

- **Gzip, CRC-32 and compression level 1.** Added RFC 1952 Gzip compression and decompression
  (`gzip_compress`, `gzip_compress_into`, `gzip_compress_bound`, `gzip_decompress`,
  `gzip_decompress_vec`), slice-by-8 IEEE `crc32` and `crc32_update`, and `Level::L1`
  (`Level::FASTEST`) hash-table compression alongside named constants `Level::L0` to
  `Level::L9`.

- **Reusable contexts and `#![no_std]`.** Added `Compressor` (reusing hash and window
  allocations across calls), zero-allocation `Decompressor`, `deflate_compress_into`,
  `zlib_compress_into`, `deflate_decompress_vec` and `zlib_decompress_vec`. The crate is now
  `#![no_std]` (using `core` and `alloc` only), and `deflate_compress_bound`,
  `zlib_compress_bound` and `gzip_compress_bound` are `const fn`.

- **Faster, zero-allocation decompression and edge-case fixes.** Replaced per-block 128 KiB
  `Vec` Huffman decode tables with compact inline two-level `DecodeTable`s and compile-time
  `const` fixed tables, added a safe 64-bit interior decode fast loop and doubling
  `copy_within` for periodic matches, and fixed empty stored block `is_final` handling and
  `>= WINDOW_SIZE` window sliding.

- Output for levels 0 and 2 to 9 and zlib/DEFLATE decompression is unchanged, checked against
  libdeflate on 30,000 sources (953 MB).

## 0.2.1

- **Stricter lints, enforced in CI.** `Cargo.toml` turns on clippy's pedantic group for every
  target, and forbids `unsafe` code in the tests, examples and benchmarks as well as the
  library. Functions whose result should not be ignored are marked `#[must_use]`, and the
  decompressors document their errors.

- **Tidier internals.** The compressor's match limits, which shrink near the end of the
  input, are a small type of their own rather than two values passed by mutable reference.

- Output is unchanged, checked against libdeflate on 30,000 sources (953 MB), and so is
  speed: the same number of instructions are executed.

## 0.2.0

- **Breaking: an idiomatic API.** Compression levels are a `Level` type that holds only
  the levels ported (0 and 2 to 9), so `zlib_compress` and `deflate_compress` no longer
  panic; they take the input first: `zlib_compress(&data, Level::DEFAULT)`. Decompression
  returns a `Decompressed` with named fields, `bytes_read` and `bytes_written`, in place of
  a tuple. `adler32(data)` starts a checksum, and `adler32_update(adler, data)` continues
  one. `MAX_LEVEL` is replaced by `Level::BEST`.

- **Faster.** Compression is 3% to 64% faster and decompression 6% to 54% faster,
  depending on the data and level (see [Performance](docs/performance.md)). Output is
  unchanged, checked against libdeflate on 40,000 sources (1.28 GB).

- **Rewritten as idiomatic Rust.** The code no longer mirrors libdeflate's C line by line.

- **Documentation.** Every public item is documented, the README is a short overview, and
  the detail is in [Guide](docs/guide.md), [Verification](docs/verification.md) and
  [Performance](docs/performance.md).

## 0.1.0

- First release: libdeflate's compressor (levels 0 and 2 to 9) and decompressor, moved out
  of [rustbcx](https://github.com/OctoKeights/rustbcx), where it was `rustbcx::libdeflate`.
