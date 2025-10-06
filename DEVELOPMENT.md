## Module-level documentation

We use [cargo_readme](https://docs.rs/cargo-readme/latest/cargo_readme/) to automatically manage the README.md.
Write all documentation that might be appropriate to go in the README.md file in the module-level rustdoc, then run `cargo readme > README.md` to update the README.md file.
Do not edit README.md directly, as it should be automatically managed this way to ensure consistency (and accuracy, considering doctests).

## Testing

To run all tests, run `cargo test --all-features` as well as `cargo test --doc`.
Some doctests have different behavior based on which features are enabled.
These doctests have top lines that look like this:

```
//! # #[cfg(not(feature = "testing"))]
//! # fn main() {}
//! # #[cfg(feature = "testing")]
//! # fn main() {
```

We should maintain that these tests pass regardless of which features are enabled.
