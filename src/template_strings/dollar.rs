use std::iter::Iterator;
use super::result::{Result, Error};
use super::item::Item;

pub struct Dollar<'a> {
    template: &'a str
}

pub fn dollar<'a, S>(s: S) -> Dollar<'a> where S: Into<&'a str> {
    Dollar {template: s.into()}
}

impl<'a> Dollar<'a> {
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

fn valid_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

impl<'a> Iterator for Dollar<'a> {
    type Item = Result<Item<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        use Item::{Literal,Variable};

        if self.template.is_empty() {
            return None;
        }

        if self.template.len() < 2 {
            return Some(Ok(Literal(self.flush())));
        }

        let (first2, rest) = self.template.split_at(2);
        if first2 == "$$" {
            self.template = rest;
            return Some(Ok(Literal("$")));
        }

        if first2 == "${" {
            if let Some((name, rest)) = rest.split_once('}') {
                self.template = rest;
                return Some(Ok(Variable(name)));
            }
            return Some(self.error("unterminated '{...'"));
        }

        let (first, rest) = self.template.split_at(1);
        if first == "$" {
            let idx = rest.find(|c| !valid_id_char(c))
                .unwrap_or(rest.len());
            let (name, rest) = rest.split_at(idx);
            if name.is_empty() {
                return Some(self.error("invalid identifier character"));
            }
            self.template = rest;
            return Some(Ok(Variable(name)));
        }

        let idx = self.template.find(|c| c == '$')
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
        let it1 = dollar("Hello");
        let re1: Result<Vec<Item>> = it1.collect();
        assert_eq!(re1.unwrap(), vec![Literal("Hello")]);

        let it2 = dollar("Hello, ${name}!");
        let re2: Result<Vec<Item>> = it2.collect();
        assert_eq!(re2.unwrap(), vec![Literal("Hello, "), Variable("name"), Literal("!")]);

        let it3 = dollar("Hello, $}name{");
        let re3: Result<Vec<Item>> = it3.collect();
        assert!(matches!(re3, Err(Error::InvalidTemplate(_))));

        let it4 = dollar("Hell, ${name");
        let re4: Result<Vec<Item>> = it4.collect();
        assert!(matches!(re4, Err(Error::InvalidTemplate(_))));

        let it5 = dollar("$$ Literal $$");
        let re5: Result<Vec<Item>> = it5.collect();
        assert_eq!(re5.unwrap(), vec![Literal("$"), Literal(" Literal "), Literal("$")]);
    }

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
        assert!(matches!(
            dollar("${name{").format(&env),
            Err(Error::InvalidTemplate(_))));
        assert!(matches!(
            dollar("${number}").format(&env),
            Err(Error::MissingVariable(n)) if n == "number"));
    }
}
