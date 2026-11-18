// ~/~ begin <<docs/src/template_strings.md#template-strings::src/template_strings/item.rs>>[init]
use super::result::{Result, Error};
use super::format::{Format, Env};

/// An `Item` refers to either a string literal or a variable
/// reference. `Item` implements the `Format` trait, with a `str`
/// output.
///
/// Any type that implements an iterator over `Item`
/// also implements the `Format` trait with a `String` output.
#[derive(Debug, PartialEq, Eq)]
pub enum Item<'a> {
    Literal(&'a str),
    Variable(&'a str)
}

// ~/~ begin <<docs/src/template_strings.md#template-strings::format-item>>[init]
impl<'a> Format<'a> for &Item<'a> {
    type Output = &'a str;
    fn format(self, env: &'a Env) -> Result<Self::Output> {
        use Item::{Literal, Variable};
        use Error::MissingVariable;

        match *self {
            Literal(x) => Ok(x),
            Variable(n) => env.get(&n.to_string()).map(String::as_str)
                .ok_or(MissingVariable(n.to_string()))
        }
    }
}

impl<'a, T: Iterator<Item=Result<Item<'a>>>> Format<'a> for T {
    type Output = String;
    fn format(self, env: &'a Env) -> Result<Self::Output> {
        let mut result = String::new();
        for i in self {
            result.push_str(i?.format(env)?);
        }
        Ok(result)
    }
}
// ~/~ end
// ~/~ end
