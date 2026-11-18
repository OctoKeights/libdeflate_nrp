# Changelog

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
