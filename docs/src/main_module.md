Main Module
===========

The crate consists of a library and a binary. The library exports the modules that make up Brei, so that their public items form its API.

```rust
//| file: src/lib.rs
pub mod template_strings;
pub mod noodles;
pub mod tasks;
pub mod data;
```

The binary is the `brei` command-line program.

```rust
//| file: src/main.rs
fn main() {
    println!("Hello, world!");
}
```
