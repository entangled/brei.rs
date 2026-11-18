# Brei.rs

Brei is a small workflow system: a replacement for GNU Make, for systems that don't have it. This is the Rust version of [Brei](https://github.com/entangled/brei), which was written in Python. It reads the same workflows, with a few [differences](https://entangled.github.io/brei.rs/compat.html).

The documentation is at [entangled.github.io/brei.rs](https://entangled.github.io/brei.rs). Brei.rs is a literate program, so this is also the source code.

```toml
[[task]]
description = "Greet the Globe"
stdout = "hello.txt"
script = "echo 'Hello, World!'"
```

- No new syntax: workflows are TOML, YAML or JSON files.
- Tasks run lazily and in parallel.
- Templates, variables, includes and configurable runners.
- A single executable, with no dependencies.

Brei was developed as part of the [Entangled project](https://entangled.github.io), for small automations in literate programming, like generating figures. For anything bigger, use a more established tool.

## Installing

```
cargo install --git https://github.com/entangled/brei.rs
```

## Usage

Brei reads `brei.toml`, `brei.yaml`, or the `[tool.brei]` section of `pyproject.toml`, whichever it finds first. Use `-i` to read another file.

```
brei all                         # build the task named `all`
brei -i examples/rot13.toml all  # read the workflow from another file
brei -j 4 -k all                 # four jobs at a time, keep going after a failure
brei --list-runners
```

A task runs when one of its targets is missing, older than one of its dependencies, or made by a different version of its script. Brei keeps that history in `.brei_history`.

## Development

Brei.rs is a literate program: the source is the book in `docs/src/`, and the Rust files in `src/` are generated from it with [Entangled](https://entangled.github.io). While Entangled is watching, you can edit either the Markdown or the Rust files, and the changes go both ways:

```
uv run entangled watch
```

- **Tests:** `cargo test`. Some tests compare with Python Brei, and need `python` and `git`.
- **Book:** `mdbook build docs` (or `mdbook serve docs`). It builds `brei`, and runs the examples marked `eval` to show their output.
- **Coverage:** `brei coverage`, with Brei's own workflow in `brei.toml`. It needs `jq` and `rustup component add llvm-tools`, and puts the HTML report in the book.

## License

Apache License 2.0, see [LICENSE](LICENSE).

## Use of generative AI

Significant parts of this port were done by Claude Opus 5.5, a large language model by Anthropic, under the direction of the author. See the [statement](https://entangled.github.io/brei.rs/#use-of-generative-ai) in the book for details.
