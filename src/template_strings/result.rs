// ~/~ begin <<docs/src/template_strings.md#template-strings::src/template_strings/result.rs>>[init]
#[derive(Debug)]
pub enum Error {
    InvalidTemplate(String),
    MissingVariable(String)
}

pub type Result<T> = core::result::Result<T, Error>;
// ~/~ end
