# Recorded results

`libdeflate_expected.txt` holds the hashes `compare/dump_libdeflate.c` prints when built
against libdeflate 1.26, for synthetic data. `tests/golden.rs` checks that libdeflate_nrp
prints the same.

To record them again, run the comparison once, which builds libdeflate's side, then run that
program directly:

    compare/compare.sh <checkout of libdeflate> 3000 1
    ref="${TMPDIR:-/tmp}/libdeflate_nrp-compare/dump-libdeflate"
    { "$ref" libdeflate 3 200; "$ref" libdeflate 8 200; } > tests/data/libdeflate_expected.txt

All test data is generated from seeds; no external files are used.
