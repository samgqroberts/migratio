## Testing

To run the full verification suite across all feature combinations, install [just](https://github.com/casey/just) and run:

```
just verify
```

You can also run individual test configurations (e.g., `just test-sqlite`) or verify compatibility with different dependency versions (`just verify-mysql-versions`, `just verify-sqlite-versions`). Run `just --list` to see all available recipes.

To run all tests manually, run `cargo test --all-features` as well as `cargo test --doc`.
Some doctests have different behavior based on which features are enabled.
These doctests have top lines that look like this:

```
//! # #[cfg(not(feature = "testing"))]
//! # fn main() {}
//! # #[cfg(feature = "testing")]
//! # fn main() {
```

We should maintain that these tests pass regardless of which features are enabled.
