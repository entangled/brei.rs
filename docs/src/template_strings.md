---
entangled:
  version: "2.7"
  namespace: template-strings
---

Template Strings
================

Brei supports template strings that expand variables that are written in a Bash-like syntax `${name}` or `$name`. Most string formatting in Rust is done compile time, so here's a small library that provides run-time template strings.

The reason we chose dollar signs over curly braces, is that we think this will require fewer character escapes in task definitions.

Some end-to-end tests:

```rust
//| id: end-to-end-test
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
```

For this submodule we define a specialized error and result type:

```rust
//| file: src/template_strings/result.rs
#[derive(Debug)]
pub enum Error {
    InvalidTemplate(String),
    MissingVariable(String)
}

pub type Result<T> = core::result::Result<T, Error>;
```

We define a `Format` trait that provides the `format` method on implemented types. We need to pass an environment containing the string replacements.

```rust
//| file: src/template_strings/format.rs
use std::collections::HashMap;
use super::result::Result;

/// The `Env` type is used for passing variable bindings to the
/// `format` method.
pub type Env = HashMap<String, String>;

pub trait Format<'a> {
    type Output;

    fn format(self, env: &'a Env) -> Result<Self::Output>;
}
```

First, we decompose a given string into `Item`s, being either a `Literal` or `Variable` part.

```rust
//| file: src/template_strings/item.rs
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

<<format-item>>
```

We implement the `Format` trait for both `Item` and `Iterator<Item=Result<Item>>>`.

```rust
//| id: format-item
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
```

Now, all we need to do is write parsers that turn a string into an iterator of items. We have an implementation for curly brace syntax that is similar to many modern programming languages, and the dollar sign syntax for Brei. We leave the curly-brace syntax in for possible later use.

Curly (rust) syntax
-------------------

```rust
//| file: src/template_strings/curly.rs
use std::iter::Iterator;
use super::result::{Result, Error};
use super::item::Item;

pub struct Curly<'a> {
    template: &'a str
}

pub fn curly<'a, S>(s: S) -> Curly<'a> where S: Into<&'a str> {
    Curly { template: s.into() }
}

impl<'a> Curly<'a> {
    fn flush(&mut self) -> &'a str {
        let (result, end) = self.template.split_at(self.template.len());
        self.template = end;
        result
    }

    fn error<T>(&mut self, msg: T) -> Result<Item<'a>>
    where T: ToString {
        self.flush();
        Err(Error::InvalidTemplate(msg.to_string()))
    }
}

impl<'a> Iterator for Curly<'a> {
    type Item = Result<Item<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        use Item::{Literal,Variable};

        if self.template.is_empty() {
            return None;
        }

        if self.template.get(..2) == Some("{{") {
            let (_, rest) = self.template.split_at(2);
            self.template = rest;
            return Some(Ok(Literal("{")));
        }

        if self.template.get(..2) == Some("}}") {
            let (_, rest) = self.template.split_at(2);
            self.template = rest;
            return Some(Ok(Literal("}")));
        }

        let (first, rest) = self.template.split_at(1);
        if first == "{" {
            if let Some((name, rest)) = rest.split_once('}') {
                self.template = rest;
                return Some(Ok(Variable(name)));
            }
            return Some(self.error("unterminated '{...'"));
        }

        if first == "}" {
            return Some(self.error("unexpected '}'"));
        }

        let idx = self.template.find(['{', '}'])
            .unwrap_or(self.template.len());
        let (result, rest) = self.template.split_at(idx);
        self.template = rest;
        Some(Ok(Literal(result)))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::super::format::{Format, Env};
    use Item::{Literal, Variable};

    #[test]
    fn test_iterator() {
        let it1 = curly("Hello");
        let re1: Result<Vec<Item>> = it1.collect();
        assert_eq!(re1.unwrap(), vec![Literal("Hello")]);

        let it2 = curly("Hello, {name}!");
        let re2: Result<Vec<Item>> = it2.collect();
        assert_eq!(re2.unwrap(), vec![Literal("Hello, "), Variable("name"), Literal("!")]);

        let it3 = curly("Hello, }name{");
        let re3: Result<Vec<Item>> = it3.collect();
        assert!(matches!(re3, Err(Error::InvalidTemplate(_))));

        let it4 = curly("Hell, {name");
        let re4: Result<Vec<Item>> = it4.collect();
        assert!(matches!(re4, Err(Error::InvalidTemplate(_))));

        let it5 = curly("}} Literal {{");
        let re5: Result<Vec<Item>> = it5.collect();
        assert_eq!(re5.unwrap(), vec![Literal("}"), Literal(" Literal "), Literal("{")]);
    }

    #[test]
    fn test_format() {
        let mut env = Env::new();
        env.insert(String::from("name"), String::from("Sara"));

        assert_eq!(
            curly("Hallo, {name}!").format(&env).unwrap(),
            "Hallo, Sara!");
        assert_eq!(
            curly("{{{name}}}").format(&env).unwrap(),
            "{Sara}");
        assert!(matches!(
            curly("{{name}").format(&env),
            Err(Error::InvalidTemplate(_))));
        assert!(matches!(
            curly("{number}").format(&env),
            Err(Error::MissingVariable(_))));
    }
}
```

Dollar syntax
-------------

The dollar syntax follows Python's `string.Template`, as used by the Python version of Brei. A variable is written `$name` or `${name}`, where a name starts with a letter or underscore, followed by letters, digits and underscores. `$$` is a literal dollar sign. Any other dollar sign is left alone, like Python's `safe_substitute` does. That way, scripts can contain things like `$(pwd)`, `$1` or `${#array[@]}`, which are not Brei variables, without escaping them. So parsing a dollar template never fails.

```rust
//| file: src/template_strings/dollar.rs
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

    <<end-to-end-test>>
}
```

Top-level module
----------------

```rust
//| file: src/template_strings/mod.rs
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
pub use curly::*;
```
