//! Run-time processing of template strings
//!

mod result;
mod format;
pub mod item;
mod curly;
mod dollar;

pub use result::*;
pub use item::*;
pub use format::*;
pub use dollar::*;
