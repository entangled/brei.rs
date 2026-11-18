# Welcome to Brei
Brei is a small workflow system. This is the Rust version of [Brei](https://github.com/entangled/brei), which was written in Python; it reads the same workflows, with a few [differences](./compat.md). The primary reason for creating Brei is to replace GNU Make, in order to be compatible on systems that are naturally deprived of this wonder of human ingenuity. In a nutshell:

```toml
[[task]]
description = "Greet the Globe"
stdout = "hello.txt"
script = "echo 'Hello, World!'"
```

- No new syntax: programmable workflows in TOML, YAML or JSON files.
- Efficient: Runs tasks lazily and in parallel.
- Feature complete: Supports templates, variables, includes and configurable runners.
- No dependencies: Brei is a single executable.
- Small codebase: Brei is around 2000 lines of Rust, written as a literate program. This book is the source code.

## Why
Why yet another workflow tool? This tool was developed as part of the [Entangled project](https://entangled.github.io), but can be used on its own. Brei is meant to perform small scale automisations for literate programming in Entangled, like generating figures, and performing computations locally. It requires no setup to work with and workflows are easy to understand by novice users. If you have any more serious needs than that, we'd recommend to use a more tried and proven system, of which there are too many to count.

## When to use
You're running a project, there's lots of odds and ends that need automisation. You'd use a `Makefile` but your friend is on Windows and doesn't have GNU Make installed. You try to ship a product that needs this, but don't want to confront people trying it for the first time with a tonne of stuff they've never heard of.

## Running
Install Brei with Cargo:

```
cargo install --git https://github.com/entangled/brei.rs
```

Then `brei` should be available as a command-line executable. Without `-i`, it reads `brei.toml`, `brei.yaml`, `brei.yml`, or the `[tool.brei]` section of `pyproject.toml`, whichever it finds first.

```bash
#| classes: [eval]
brei --help
```

## How it works
You give Brei a list of tasks that may depend on one another. Brei will run these when input files are newer than the target, or when the script that creates the target changed since the last run (Brei keeps a history in `.brei_history`). Execution is lazy and in parallel.

### Tasks
Tasks are the elemental units of work in Brei. A task is the single execution of a given script, and can be indicated to depend on previous tasks by explicitly listing targets and dependencies.

```toml
#| file: examples/tasks.toml
[[task]]
creates = ["hello.txt"]
runner = "bash"
script = "echo 'Hello, World!' > hello.txt"

[[task]]
name = "all"
requires = ["hello.txt"]

[[task]]
name = "clean"
script = "rm hello.txt"
```

This defines to named tasks `all` and `clean`, where `all` depends on the creation of a file `hello.txt`. Giving a `name` to a task is similar to creating a 'phony' target in Make.

### Templates
We can use patterns to create reusable items. Variables are written like in many scripting languages: `${var_name}` (or `$var_name`) substitutes for the contents of the `var_name` variable. Use two dollar signs `$$` to make a `$` literal. Any other dollar sign is left alone, so scripts can use things like `$(pwd)` or `$1` without escaping them.

```toml
#| file: examples/rot13.toml
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
```

```bash
#| classes: [eval]
brei -i examples/rot13.toml all
```

### Multiplexing

You can call templates with lists of arguments to create many tasks. There are two ways to combine multiple arguments: `inner` and `outer`, configured with the `join` argument. The `inner` product takes the first item of each list, then the second, and so on, while `outer` takes every combination. The default is `inner`.

```toml
#| file: examples/template_multiplexing.toml
[[task]]
description = "Creating temporary directory"
stdout = "var(dir)"
script = "mktemp -d"

[template.touch]
description = "${pre} ${a} ${b}"
creates = ["${dir}/${pre}-${a}-${b}"]
script = "touch '${dir}/${pre}-${a}-${b}'"

[[call]]
template = "touch"
collect = "inner"
  [call.args]
  pre = "inner"
  a = ["x", "y", "z"]
  b = ["1", "2", "3"]

[[call]]
template = "touch"
collect = "outer"
join = "outer"
  [call.args]
  pre = "outer"
  a = ["x", "y"]
  b = ["1", "2"]

[[task]]
name = "all"
requires = ["#inner", "#outer"]
```

The `collect` argument creates a collection phony task containing all items in the call.

```bash
#| classes: [eval]
brei -i examples/template_multiplexing.toml all
```

### Variables
You may write the output of a command to the contents of a variable, by using `"var(name)"` as a target.
For instance, in many science applications its desirable to know which version of a software generated some output. 

```toml
#| file: examples/versioned_output.toml
[environment]
data_dir = "./data"
output_dir = "./output/${commit}"

[[task]]
stdout = "var(commit)"
script = "git rev-parse HEAD"

[[task]]
creates = ["${output_dir}/data.h5"]
requires = ["${data_dir}/input.h5", "#prepare"]
runner = "python"
path = "scripts/run.py"

[[task]]
name = "prepare"
script = """
mkdir -p ${output_dir}
ln -sf ${output_dir} output/latest
"""

[[task]]
name = "all"
requires = ["${output_dir}/data.h5"]
```

Also note the following:

- Instead of listing a `script` you can give a `path` to an existing script.
- Named (phony) targets are referenced with a hash symbol `#`.
- Tasks with no targets are always run, and so are tasks that write to a variable.
- The `[environment]` item lists global variables.
- All string substitution is done lazily.

### List variables
A task can also write its output to a list variable, by using `"listvar(name)"` as `stdout`. Each line of the output becomes an element of the list; empty lines are skipped. You can then use `"splice(name)"` as an element of a list in the arguments of a template call, to create a task for each element, or in `requires`, to depend on all of them. For instance, to copy all figures to the website:

```toml
#| file: examples/list_variables.toml
[[task]]
description = "Listing figures"
stdout = "listvar(figures)"
runner = "python"
script = """
from pathlib import Path
for p in sorted(Path("docs/fig").glob("*.svg")):
    print(p.name)
"""

[template.copy]
description = "copy `${name}`"
requires = ["docs/fig/${name}"]
creates = ["docs/site/fig/${name}"]
runner = "python"
script = """
import shutil
shutil.copy("docs/fig/${name}", "docs/site/fig/${name}")
"""

[[call]]
template = "copy"
collect = "figures"
args = { name = ["splice(figures)"] }

[[task]]
name = "all"
requires = ["#figures"]
```

The task that lists the figures has no file targets, so it runs every time, and new figures are found. Each copy task only runs when its figure changed.

List variables are kept simple on purpose:

- `listvar(name)` can only be used as `stdout` of a task.
- `splice(name)` can only be a whole element of a list in `requires` or in template call arguments, not part of a string.
- A list variable can't be used in a string with `${name}`.
- In an `inner` join, all lists should have the same length.

Brei has no functions to work on file names. Instead, have the listing print what the template needs, like the file names above. For anything more involved, generate a workflow and include it, see the next section.

### Includes
You can include parts of a workflow from other files, in TOML, YAML or JSON.

```toml
#| file: examples/echo.toml
[template.echo]
stdout = "${stdout}"
script = "echo '${text}'"
```

```toml
#| file: examples/hello-includes.toml
include = [
    "./echo.toml"
]

[[call]]
template = "echo"
  [call.args]
  stdout = "hello.txt"
  text = "Hello, World!"

[[task]]
name = "all"
requires = ["hello.txt"]
```

It is even possible to include files that still need to be generated. The following generates a file with ten tasks, includes that file and runs those tasks.

```toml
#| file: examples/include-gen.toml
include = [
    "${dir}/gen.json"
]

[[task]]
description = "Creating temporary directory"
stdout = "var(dir)"
script = "mktemp -d"

[[task]]
description = "Generating workflow"
stdout = "${dir}/gen.json"
runner = "python"
script = """
import json
tasks = [
    {"stdout": f"${dir}/out{i}.dat",
     "script": f"echo '{i}'"} for i in range(10)
]
tasks.append({"name": "write-outs", "requires": [
    f"${dir}/out{i}.dat" for i in range(10)
]})
print(json.dumps({"task": tasks}))
"""

[[task]]
name = "all"
requires = ["#write-outs"]
```

```bash
#| classes: [eval]
brei -i examples/include-gen.toml all
```

### Custom Runner
By default, the contents of `script` is split in lines, each line is split into arguments like a Unix shell would, and run as a command directly, without a shell. What that means is that the script will perform the same operations on all platforms, but there are no pipes, redirections or shell variables. However, you can choose to have the script run by any other means by providing the `runner` argument.


```toml
#| file: examples/custom-runner.toml
[[task]]
description = "Creating temporary directory"
stdout = "var(dir)"
script = "mktemp -d"

[runner.lua]
command = "lua"
args = ["${script}"]

[[task]]
runner = "lua"
stdout = "${dir}/hello.txt"
script = """
function fact (n)
  if n == 0 then
    return 1
  else
    return n * fact(n-1)
  end
end

print("10! = ", fact(10))
"""

[[task]]
name = "all"
requires = ["${dir}/hello.txt"]
script = "cat ${dir}/hello.txt"
```

```bash
#| classes: [eval]
brei -i examples/custom-runner.toml all
```

There are a number of runners configured by default.

```bash
#| classes: [eval]
brei --list-runners
```

### Forcing a rerun
Sometimes you want to always rerun a task no matter what. Brei measures its own test coverage this way, with this `brei.toml`. The tests run every time, also when no source file changed: they use other programs too, like `bash` and `python`, and Brei can't see when those change. The other tasks run whenever there is a new profile.

The LLVM tools that read the profiles have to be the same version as the LLVM in `rustc`, so we use the ones that come with it (install them with `rustup component add llvm-tools`). Every process that runs during the tests (the test binaries, and the `brei` binary that some of them run) writes a raw profile of its own, and these are merged into one.

```toml
#| file: brei.toml
[environment]
ignore = "/.cargo/registry|/lib/rustlib/"

[[task]]
name = "coverage"
description = "Coverage report"
requires = ["docs/src/coverage/index.html"]
script = "${llvm_bin}/llvm-cov report --ignore-filename-regex=${ignore} --instr-profile=cov/brei.profdata ${objects}"

[[task]]
description = "Finding the LLVM tools"
stdout = "var(llvm_bin)"
runner = "bash"
script = '''echo "$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"'''

[[task]]
description = "Running tests with coverage"
creates = ["cov/brei.profdata"]
force = true
runner = "bash"
script = '''
set -e
rm -rf cov/profraw
LLVM_PROFILE_FILE="$(pwd)/cov/profraw/%p-%m.profraw" RUSTFLAGS="-C instrument-coverage" cargo test
${llvm_bin}/llvm-profdata merge -sparse cov/profraw/*.profraw -o cov/brei.profdata
'''

[[task]]
description = "Listing instrumented executables"
stdout = "var(objects)"
requires = ["cov/brei.profdata"]
runner = "bash"
script = '''
LLVM_PROFILE_FILE=/dev/null RUSTFLAGS="-C instrument-coverage" cargo test --no-run --message-format=json \
    | jq -j 'select(.reason == "compiler-artifact" and .executable != null) | "-object \(.executable) "'
'''

[[task]]
description = "Writing HTML report"
requires = ["cov/brei.profdata"]
creates = ["docs/src/coverage/index.html"]
script = "${llvm_bin}/llvm-cov show --format=html --project-title=Brei --output-dir=docs/src/coverage --ignore-filename-regex=${ignore} --instr-profile=cov/brei.profdata ${objects}"

[[task]]
name = "clean"
script = "rm -rf cov docs/src/coverage"
```

The HTML report goes into the source of this book, so that it becomes part of the [coverage chapter](./coverage.md).

To pass the executables to `llvm-cov`, we list them by building the tests again, which is quick because they are already built. Build scripts of dependencies may still run in this step, and would leave profiles of their own in the current directory, so those go to `/dev/null`.

```bash
brei coverage
```

### Remarks

- If you need your workflows also execute on a Windows machine, it is advised to write scripts for the default runner (lists of commands) or in a language like Python.
- Brei is not meant for building programs, so it doesn't have the same feature set as GNU Make. If you need more complex logic, you can write a Brei generator. The generator creates tasks, writes them to JSON and then Brei can `include` the result from your generator task.
- TOML is nice but not ideal: it can be tricky to see the difference beteween single `[...]` and double `[[...]]` square brackets. Sometimes TOML syntax will lead to very verbose notation, however, current alternatives are all worse.
- Many modern programming languages that we like (Python, Rust, Julia) have their project settings in a TOML file. This way your Brei workflow can piggy-back on project files that are already there.

## License
Copyright 2023 Netherlands eScience Center, Licensed under the Apache License, Version 2.0, see [LICENSE](https://github.com/entangled/brei.rs/blob/main/LICENSE).

### Use of generative AI
Significant parts of this port from Python to Rust were done by Claude Opus 5.5, a large language model by Anthropic, working in [Claude Code](https://claude.com/claude-code). It worked under the direction of the author, who set the design and the conventions. The first version of the task engine (`noodles`), the template strings, and the literate setup of the project were written by hand. Claude wrote most of the rest: the engine's memoization, cancellation and cycle detection; the Brei rules, variables, templates and program resolution; the program loader and the command-line interface; the tests ported from Python Brei; and this book's tooling for coverage and running examples. Most of the prose in this book was written by Claude as well. Commits with Claude's work are marked `Co-Authored-By: Claude` in the Git history.
