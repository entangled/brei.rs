// ~/~ begin <<docs/src/template_strings.md#template-strings::src/template_strings/curly.rs>>[init]
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
        return result;
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

        let idx = self.template.find(|c| c == '{' || c == '}')
            .unwrap_or(self.template.len());
        let (result, rest) = self.template.split_at(idx);
        self.template = rest;
        return Some(Ok(Literal(result)))
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
// ~/~ end
