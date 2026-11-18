# Workflow Reference

This chapter lists everything that can go in a Brei workflow file. The [introduction](./index.md) shows the same features by example; the [Programs](./program.md) and [Brei](./brei.md) chapters explain how they work.

## Files

A workflow is a TOML, YAML (`.yaml` or `.yml`) or JSON file; the extension decides which. `brei` reads, in this order:

1. the file given with `-i`, for example `-i workflow.toml`;
2. `brei.toml` in the current directory;
3. `brei.yaml` in the current directory;
4. the `[tool.brei]` section of `pyproject.toml`.

A workflow can also be a section of a bigger file. On the command line, the section goes in brackets after the file name, with dots between the keys: `-i pyproject.toml[tool.brei]`. Included files are always read as a whole.

Paths in a workflow, including those of includes, are relative to the directory `brei` runs in. Unknown keys and values of the wrong type are errors, so that typos don't go unnoticed.

## Top-level keys

All keys are optional.

| Key           | Type                    | Meaning                                          |
|---------------|-------------------------|--------------------------------------------------|
| `task`        | list of [tasks](#tasks) | The tasks, written `[[task]]` in TOML.           |
| `environment` | table of strings        | [Variables](#environment) with a fixed value.    |
| `template`    | table of [tasks](#tasks)| [Templates](#templates), by name.                |
| `call`        | list of [calls](#calls) | Uses of templates, written `[[call]]` in TOML.   |
| `include`     | list of strings         | Other workflow files to [include](#includes).    |
| `runner`      | table of [runners](#runners) | Extra runners, by name.                     |

The same workflow in TOML, YAML and JSON:

```toml
[environment]
greeting = "Hello"

[[task]]
name = "all"
stdout = "hello.txt"
script = "echo '${greeting}, World!'"
```

```yaml
environment:
  greeting: Hello
task:
  - name: all
    stdout: hello.txt
    script: "echo '${greeting}, World!'"
```

```json
{
  "environment": { "greeting": "Hello" },
  "task": [
    { "name": "all", "stdout": "hello.txt", "script": "echo '${greeting}, World!'" }
  ]
}
```

## Targets

Everything a task creates or requires is a *target*, written as a string:

| Written as      | Target                                                                  |
|-----------------|-------------------------------------------------------------------------|
| `#name`         | A *phony* target: a name for a task that doesn't create a file.         |
| `var(name)`     | A *scalar variable*: a string, computed by a task.                      |
| `listvar(name)` | A *list variable*: a list of strings, computed by a task.               |
| `splice(name)`  | The elements of the list variable `name`, as separate targets. Only in `requires` and call arguments. |
| anything else   | A *path* to a file. A leading `./` is ignored: `./a.txt` is `a.txt`.    |

Variable names can't contain white space or parentheses. `var(x)` and `listvar(x)` are the same variable, so using a variable both ways is an error.

## Template strings

Almost every string in a workflow is a template string. `${name}` (or `$name`) is replaced with the value of the scalar variable `name`. A variable gets its value from the [environment](#environment), from a task with `var(name)` as `stdout`, or from the arguments of a [call](#calls).

`$$` is a literal dollar sign. Any other dollar sign is left alone, so scripts can use things like `$(pwd)`, `$1` or `${#array[@]}` without escaping them. A name starts with a letter or underscore, followed by letters, digits and underscores.

Values are filled in lazily: a task that uses a variable waits for the task that computes it.

## Tasks

A task runs a script to create its targets from its dependencies. All keys are optional.

| Key           | Type            | Meaning |
|---------------|-----------------|---------|
| `creates`     | list of targets | What the task creates. |
| `requires`    | list of targets | What the task needs before it can run. |
| `name`        | string          | Makes the task available as the phony target `#name`. |
| `description` | string          | Shown in the log when the task runs. |
| `script`      | string          | What to run. Without a `runner`, every line is a command. |
| `runner`      | string          | The [runner](#runners) that runs `script` or `path`. |
| `path`        | string          | A script file to run, instead of `script`. Needs a `runner`, and is a dependency of the task. |
| `stdin`       | target          | Standard input: a file or a scalar variable. |
| `stdout`      | target          | Standard output: a file, `var(name)` or `listvar(name)`. It is also a target of the task. |
| `force`       | boolean         | Always run the task. Default `false`. |

### Scripts

Without a runner, every non-empty line of `script` is a command. A line is split into arguments like a Unix shell would (with quotes and backslashes), but it doesn't go through a shell: there are no pipes, redirections or shell variables. Commands run in the directory `brei` runs in.

With a runner, the script is written to a temporary file, and the runner runs that file. A task with neither `script` nor `path` doesn't run anything; it only waits for its dependencies, which is useful with `name`.

A command that doesn't read `stdin` gets empty input, not the terminal. Output that doesn't go to `stdout` (and all of standard error) goes to the log.

### Output to variables

With `stdout = "var(name)"`, the output of the script, without white space at the start and end, is the value of `name`. With `stdout = "listvar(name)"`, every non-empty line of the output is an element of the list `name`.

### When a task runs

A task runs when it is needed for the targets on the command line, and:

- it creates no files (only phony targets or variables);
- it writes its `stdout` to a variable;
- `force` is `true`, or `brei` runs with `-B`;
- one of the files it creates is missing, or older than one of the files it requires;
- or its `script` changed since the files were made. Brei keeps track of this in `.brei_history`.

After running, every file in `creates` should exist and be up to date; otherwise the task failed.

### Rules for tasks

These are errors:

- two tasks that create the same target;
- both `script` and `path`; `path` without `runner`; `runner` without `script` or `path`;
- `stdin` that is a phony target or a list variable; `stdout` that is a phony target;
- `stdin` on a script of more than one line without a runner (use a runner instead);
- `listvar(...)` anywhere but `stdout`; `splice(...)` anywhere but `requires`;
- variables in `creates`, `stdout` or `name`, unless they can be computed before running anything else (see [Variables in targets](#variables-in-targets)).

### Variables in targets

The targets of a task can use variables, as long as Brei can compute those first: the variables must come from the environment, or from tasks whose targets have no variables themselves. These tasks run while the workflow is read, once:

```toml
[[task]]
stdout = "var(dir)"
script = "mktemp -d"

[[task]]
name = "all"
stdout = "${dir}/hello.txt"
script = "echo hello"
```

## Environment

`[environment]` gives variables a value. The values are template strings, and can use other variables:

```toml
[environment]
data_dir = "./data"
output_dir = "${data_dir}/output"
```

## Templates

A template is a task with variables, that is filled in by a [call](#calls). It has the same keys as a [task](#tasks). Templates are defined by name, under `[template.name]`; when two have the same name, the last one counts.

```toml
[template.echo]
stdout = "${stdout}"
script = "echo ${text}"
```

## Calls

A call fills in a template.

| Key        | Type                                | Meaning |
|------------|-------------------------------------|---------|
| `template` | string                              | The name of the template. Required. |
| `args`     | table of strings or lists of strings| The values of the template's variables. Required. |
| `collect`  | string                              | A phony target `#collect` that requires all targets of the call. |
| `join`     | `"inner"` or `"outer"`              | How to combine list arguments. Default `"inner"`. |

The arguments are template strings themselves, so they can use variables from the environment. Variables of the template that aren't in `args` keep their meaning: they come from the environment or from tasks.

When arguments are lists, the template is filled in several times:

- `inner`: the first element of every list, then the second, and so on. All lists must have the same length.
- `outer`: every combination of elements.

An element `splice(name)` in an argument list is replaced by the elements of the list variable `name`. A string argument `splice(name)` becomes that list.

```toml
[[call]]
template = "echo"
collect = "greetings"
args = { stdout = ["hello.txt", "bye.txt"], text = ["Hello", "Bye"] }
```

The template can also come from an [include](#includes).

## Includes

`include` lists other workflow files, in TOML, YAML or JSON, whose contents are added to this one. Include paths are template strings, and an included file can be created by a task in the including workflow: Brei runs that task first.

```toml
include = ["${dir}/generated.json"]
```

Includes are read after the tasks and calls of the workflow that includes them, so a call can use a template from an include.

## Runners

A runner is a command that runs a script file.

| Key       | Type            | Meaning |
|-----------|-----------------|---------|
| `command` | string          | The program to run. |
| `args`    | list of strings | Its arguments. `${script}` is the path of the script file. Other variables are an error. |

Brei comes with two runners, `bash` and `python`, both called with `${script}` as their only argument. A runner with the same name replaces it. `brei --list-runners` shows them.

```toml
[runner.lua]
command = "lua"
args = ["${script}"]
```

## Differences with Python Brei

Brei.rs reads the same workflows as Python Brei. The few differences are listed in the [Compatibility](./compat.md) chapter.
