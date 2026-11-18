// ~/~ begin <<docs/src/template_strings.md#template-strings::src/template_strings/mod.rs>>[init]
//! Run-time processing of template strings
//!

mod result;
mod format;
pub mod item;
// Brei uses the dollar syntax; the curly syntax is kept for later use.
#[allow(dead_code)]
mod curly;
mod dollar;

pub use result::*;
pub use item::*;
pub use format::*;
pub use dollar::*;
// ~/~ end
