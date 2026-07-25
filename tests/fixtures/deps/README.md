# deps fixture

`src/lib.rs` makes this file the crate documentation with
`#![doc = include_str!("../README.md")]`, which is how a large part of the
registry documents itself. The example below is compiled as a doctest, so the
dependency it uses is used — and it is named nowhere else in the package.

```rust
use readme_crate::greet;

greet();
```
