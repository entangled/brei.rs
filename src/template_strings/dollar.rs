// ~/~ begin <<docs/src/template_strings.md#template-strings::src/template_strings/dollar.rs>>[init]
use std::iter::Iterator;
use super::result::Result;
use super::item::Item;

/// Iterates `Item` from a `&str` reference using a `${...}` type
/// string template.
pub struct Dollar<'a> {
    template: &'a str
}

/// Create an `Item` iterator for a given dollar-templated string.
pub fn dollar<'a, S>(s: S) -> Dollar<'a> where S: Into<&'a str> {
    Dollar {template: s.into()}
}

/// The length of the identifier at the start of `s`, if there is one.
fn identifier(s: &str) -> Option<usize> {
    let first = s.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    Some(s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(s.len()))
}

impl<'a> Iterator for Dollar<'a> {
    type Item = Result<Item<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        use Item::{Literal,Variable};

        if self.template.is_empty() {
            return None;
        }

        let Some(rest) = self.template.strip_prefix('$') else {
            let idx = self.template.find('$').unwrap_or(self.template.len());
            let (result, rest) = self.template.split_at(idx);
            self.template = rest;
            return Some(Ok(Literal(result)));
        };

        if let Some(rest) = rest.strip_prefix('$') {
            self.template = rest;
            return Some(Ok(Literal("$")));
        }

        if let Some(inner) = rest.strip_prefix('{') {
            if let Some(n) = identifier(inner) && inner[n..].starts_with('}') {
                self.template = &inner[n + 1..];
                return Some(Ok(Variable(&inner[..n])));
            }
        } else if let Some(n) = identifier(rest) {
            self.template = &rest[n..];
            return Some(Ok(Variable(&rest[..n])));
        }

        // not a variable, so a literal dollar sign
        self.template = rest;
        Some(Ok(Literal("$")))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::super::format::{Format, Env};
    use super::super::result::Error;
    use Item::{Literal, Variable};

    #[test]
    fn test_iterator() {
        let it1 = dollar("Hello");
        let re1: Result<Vec<Item>> = it1.collect();
        assert_eq!(re1.unwrap(), vec![Literal("Hello")]);

        let it2 = dollar("Hello, ${name}!");
        let re2: Result<Vec<Item>> = it2.collect();
        assert_eq!(re2.unwrap(), vec![Literal("Hello, "), Variable("name"), Literal("!")]);

        let it3 = dollar("$1 $(pwd) ${a b} ${#x[@]} ${name 5$");
        let re3: Result<Vec<Item>> = it3.collect();
        let text: String = re3.unwrap().iter().map(|i| match i { Literal(s) => *s, Variable(_) => panic!() }).collect();
        assert_eq!(text, "$1 $(pwd) ${a b} ${#x[@]} ${name 5$");

        let it4 = dollar("$a1_b-c ${_x}y");
        let re4: Result<Vec<Item>> = it4.collect();
        assert_eq!(re4.unwrap(), vec![Variable("a1_b"), Literal("-c "), Variable("_x"), Literal("y")]);

        let it5 = dollar("$$ Literal $$");
        let re5: Result<Vec<Item>> = it5.collect();
        assert_eq!(re5.unwrap(), vec![Literal("$"), Literal(" Literal "), Literal("$")]);
    }

    // ~/~ begin <<docs/src/template_strings.md#template-strings::end-to-end-test>>[init]
    #[test]
    fn test_format() {
        let mut env = Env::new();
        env.insert(String::from("name"), String::from("Sara"));

        assert_eq!(
            dollar("Hallo, ${name}!").format(&env).unwrap(),
            "Hallo, Sara!");
        assert_eq!(
            dollar("Hallo, $name!").format(&env).unwrap(),
            "Hallo, Sara!");
        assert_eq!(
            dollar("$${${name}}").format(&env).unwrap(),
            "${Sara}");
        assert_eq!(
            dollar("${name{").format(&env).unwrap(),
            "${name{");
        assert!(matches!(
            dollar("${number}").format(&env),
            Err(Error::MissingVariable(n)) if n == "number"));
    }
    // ~/~ end
}
// ~/~ end
