# Recorded results

`libdeflate_expected.txt` is what libdeflate makes of made-up data, as
`compare/dump_libdeflate.c` prints it; `tests/golden.rs` checks that the port prints the same.
To make it again, run the comparison once (it builds the original's side), then that by
itself:

    compare/compare.sh <checkout of libdeflate> 3000 1
    ref="${TMPDIR:-/tmp}/libdeflate_nrp-compare/dump-libdeflate"
    { "$ref" libdeflate 3 200; "$ref" libdeflate 8 200; } > tests/data/libdeflate_expected.txt

No picture or file from anywhere is in this repository: every file and stream the tests and
the comparison use is made up from a seed.
