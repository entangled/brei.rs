use std::collections::HashMap;
use super::result::Result;

pub type Env = HashMap<String, String>;

pub trait Format<'a> {
    type Output;

    fn format(self, env: &'a Env) -> Result<Self::Output>;
}
