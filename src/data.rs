use std::option::Option;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
struct Runner {
    command: String,
    args: Vec<String>
}

#[derive(Debug, Serialize, Deserialize)]
struct ProtoTask {
    #[serde(default)]
    creates: Vec<String>,

    #[serde(default)]
    requires: Vec<String>,

    #[serde(default)]
    name: Option<String>,

    #[serde(default)]
    runner: Option<String>,

    #[serde(default)]
    path: Option<String>,

    #[serde(default)]
    script: Option<String>,

    #[serde(default)]
    stdin: Option<String>,

    #[serde(default)]
    stdout: Option<String>,

    #[serde(default)]
    description: Option<String>,

    #[serde(default)]
    force: bool
}

#[derive(Debug, Serialize, Deserialize)]
enum Join {
    #[serde(alias="inner", alias="INNER")]
    Inner,

    #[serde(alias="outer", alias="OUTER")]
    Outer
}

impl Default for Join {
    fn default() -> Join {
        Join::Inner
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Arguments {
    Single(String),
    Multiple(Vec<String>)
}

#[derive(Debug, Serialize, Deserialize)]
struct Call {
    template: String,

    args: HashMap<String, Arguments>,

    #[serde(default)]
    collect: Option<String>,

    #[serde(default)]
    join: Join
}

#[derive(Debug, Serialize, Deserialize)]
struct Program {
    #[serde(default)]
    task: Vec<ProtoTask>,

    #[serde(default)]
    environment: HashMap<String, String>,

    #[serde(default)]
    template: HashMap<String, ProtoTask>,

    #[serde(default)]
    call: Vec<Call>,

    #[serde(default)]
    include: Vec<String>,

    #[serde(default)]
    runner: HashMap<String, Runner>
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml;

    #[test]
    fn test_112() {
        assert_eq!(1 + 1, 2);
    }

    const TEST_PROTOTASK: &str = r#"
        description = "Creating temporary directory"
        stdout = "var(dir)"
        script = "mktemp -d"
        "#;

    #[test]
    fn test_task_de() {
        let p: ProtoTask = serde_json::from_str("{ \"name\": \"hello\" }").unwrap();
        assert_eq!(p.name, Some(String::from("hello")));
        let q: ProtoTask = toml::from_str("name = \"hello\"").unwrap();
        assert_eq!(q.name, Some(String::from("hello")));
        let s: ProtoTask = toml::from_str(TEST_PROTOTASK).unwrap();
        assert_eq!(s.script, Some(String::from("mktemp -d")));
    }

    const TEST_CFG: &str = r#"
        [[task]]
        description = "Creating temporary directory"
        stdout = "var(dir)"
        script = "mktemp -d"

        [template.echo]
        stdout = "${stdout}"
        script = "echo ${text}"

        [template.rot13]
        stdout = "${stdout}"
        stdin = "${stdin}"
        script = "tr a-zA-Z n-za-mN-ZA-M"

        [[call]]
        template = "echo"
            [call.args]
            stdout = "${dir}/secret.txt"
            text = "Uryyb, Jbeyq!"

        [[call]]
        template = "rot13"
            [call.args]
            stdin = "${dir}/secret.txt"
            stdout = "${dir}/msg.txt"

        [[task]]
        name = "all"
        requires = ["${dir}/msg.txt"]
        script = "cat ${dir}/msg.txt"
    "#;

    #[test]
    fn read_sample() {
        let prg: Program = toml::from_str(TEST_CFG).unwrap();
        assert_eq!(prg.task[0].description, Some(String::from("Creating temporary directory")));
    }
}
