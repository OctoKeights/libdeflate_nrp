# Performance

[Overview](../README.md) · [Guide](guide.md) · [Verification](verification.md) · **Performance**

---

`examples/bench.rs` compresses and decompresses 4 MiB of synthetic text-like data (words
from a small vocabulary) and binary-like data (runs, repeats and noise) at several levels,
and reports the best of seven runs:

```sh
cargo run --release --example bench
```

## Throughput

Measured on one core of an Intel Xeon at 2.1 GHz, built for generic x86-64, in megabytes of
uncompressed data per second:

| Data | Level | Output size | Compress | Decompress |
|---|---|---|---|---|
| Text-like | 0 | 100.0% | 2,190 MB/s | 2,360 MB/s |
| Text-like | 2 | 23.5% | 71 MB/s | 301 MB/s |
| Text-like | 4 | 21.5% | 57 MB/s | 352 MB/s |
| Text-like | 6 | 20.2% | 35 MB/s | 406 MB/s |
| Text-like | 9 | 18.1% | 5 MB/s | 432 MB/s |
| Binary-like | 0 | 100.0% | 2,170 MB/s | 2,430 MB/s |
| Binary-like | 2 | 15.2% | 131 MB/s | 348 MB/s |
| Binary-like | 4 | 14.7% | 123 MB/s | 365 MB/s |
| Binary-like | 6 | 13.7% | 105 MB/s | 382 MB/s |
| Binary-like | 9 | 13.7% | 44 MB/s | 401 MB/s |

Level 9 is slow on the text-like data because its small vocabulary fills the hash chains
with candidates, all of which level 9 searches.

## Compared with 0.1

Version 0.2 rewrote the crate as idiomatic Rust. Measured alternately with 0.1 on the same
machine:

| Data | Level | Compress | Decompress |
|---|---|---|---|
| Text-like | 2 | 20% faster | 37% faster |
| Text-like | 4 | 16% faster | 41% faster |
| Text-like | 6 | 9% faster | 54% faster |
| Text-like | 9 | 11% faster | 52% faster |
| Binary-like | 2 | 64% faster | 43% faster |
| Binary-like | 4 | 38% faster | 7% faster |
| Binary-like | 6 | 11% faster | 8% faster |
| Binary-like | 9 | 3% faster | 6% faster |

Level 0 is unchanged in both directions: it is limited by copying memory.

Most of the gain in decompression comes from copying non-overlapping matches as whole
slices; most of the gain in compression from bounds checks the compiler can now remove.

## Compared with libdeflate

libdeflate itself is considerably faster at decompression, because its fast loop decodes
several symbols per refill and copies matches a word at a time with unchecked memory access.
This crate uses only libdeflate's careful loop (see the [Guide](guide.md#differences-from-libdeflate)),
so its decompression runs at roughly a third to a half of libdeflate's speed.
