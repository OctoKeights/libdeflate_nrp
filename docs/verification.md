# Verification

[Overview](../README.md) · [Guide](guide.md) · **Verification** · [Performance](performance.md)

---

libdeflate_nrp is checked against libdeflate itself: both are run over the same synthetic
data and their output compared byte for byte.

## The comparison

`compare/compare.sh` builds libdeflate from a checkout, builds `compare/dump_libdeflate.c`
against it and `examples/dump.rs` against libdeflate_nrp, runs both, and compares what they
print:

```sh
compare/compare.sh <checkout of libdeflate 1.26> [count] [seeds]
```

OpenEXR 3.5's `external/deflate` folder is the same libdeflate and works too.

Both programs generate the same data from each seed and print a running hash of everything
they produce:

- **Compression** of synthetic data of eight kinds and many sizes, sometimes several pieces
  joined, into zlib streams at levels 0 and 2 to 9.

- **Decompression** of each stream intact, into too little room and into more, damaged,
  truncated, and with bytes after it; and of random bytes after a valid zlib header.

## Results

| Sources | Data | Result |
|---|---|---|
| 40,000 | 1.28 GB | Every stream identical; every decompression gives the same bytes or the same error |

## Tests

`cargo test` needs no C compiler. `tests/golden.rs` checks libdeflate_nrp against hashes
recorded from libdeflate (`tests/data/libdeflate_expected.txt`, 400 sources), unit tests
check the length and offset tables and round trips at every level, and the examples in the
README and the Guide are run as tests. `tests/data/README.md` explains how to record the
hashes again.
