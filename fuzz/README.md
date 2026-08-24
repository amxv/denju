# Property and fuzz checks

Denju keeps its pure untrusted-input fuzz surface in ordinary Rust integration tests powered by
`proptest`. This avoids requiring a machine-global `cargo-fuzz` installation while retaining
shrinking and reproducible regression cases.

Run the bounded extended corpus with:

```sh
cargo xtask fuzz
```

The default is 4096 cases per property. Set `DENJU_PROPTEST_CASES` to another bounded value when
investigating a failure. Minimized `proptest` regression files produced by failures should be
committed alongside the affected property test.
