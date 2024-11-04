use std::collections::HashMap;
use super::result::Result;

/// The `Env` type is used for passing variable bindings to the
/// `format` method.
pub type Env = HashMap<String, String>;

pub trait Format<'a> {
    type Output;

    fn format(self, env: &'a Env) -> Result<Self::Output>;
}
