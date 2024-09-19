#[derive(Debug)]
pub enum Error {
    InvalidTemplate(String),
    MissingVariable(String)
}

pub type Result<T> = core::result::Result<T, Error>;
