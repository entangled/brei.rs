#!/usr/bin/env python3
"""mdBook preprocessor that runs code blocks marked with the `eval` class.

In the Entangled "basic" style, a code block gets its attributes from a
header of `#| key: value` lines:

    ```bash
    #| classes: [eval]
    brei --help
    ```

Such a block is run with Bash, and its output (stdout and stderr together)
is added as a `text` block right after it. The header is removed. For
styling, the command gets the class `eval` and the output `eval-output`
(mdBook turns `bash,eval` into the classes `language-bash eval`). Blocks run
in order, in one scratch directory per book, so later blocks see the files
that earlier ones made. A block that fails is still shown, with its exit
code, and gives a warning.

Configuration, in `book.toml`:

    [preprocessor.eval]
    command = "python3 ../scripts/mdbook_eval.py"
    setup = "cargo build --quiet"    # run in the book root first
    path = ["../target/debug"]       # put in front of PATH
    copy = ["../examples"]           # copied into the scratch directory
    timeout = 60                     # seconds per block

Paths are relative to the book root.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

FENCE = re.compile(r"^(?P<indent>\s*)(?P<fence>`{3,}|~{3,})\s*(?P<info>.*)$")
HEADER = re.compile(r"^\s*#\|\s*(?P<key>[\w-]+)\s*:\s*(?P<value>.*?)\s*$")
SHELLS = {"bash", "sh"}


def warn(msg):
    print(f"[eval] {msg}", file=sys.stderr)


def parse_value(value):
    if value.startswith("[") and value.endswith("]"):
        return [v.strip().strip("'\"") for v in value[1:-1].split(",") if v.strip()]
    return value.strip("'\"")


def split_header(lines):
    """Split the `#|` header off the lines of a code block."""
    attrs = {}
    n = 0
    for line in lines:
        m = HEADER.match(line)
        if not m:
            break
        attrs[m["key"]] = parse_value(m["value"])
        n += 1
    return attrs, lines[n:]


class Evaluator:
    def __init__(self, root, config):
        self.root = Path(root)
        self.config = config
        self.workdir = None
        self.env = None

    def setup(self):
        """Prepare the scratch directory, the first time a block runs."""
        if self.workdir is not None:
            return
        if setup := self.config.get("setup"):
            subprocess.run(setup, shell=True, cwd=self.root, check=True)
        self.tempdir = tempfile.TemporaryDirectory(prefix="mdbook-eval-")
        self.workdir = Path(self.tempdir.name)
        for entry in self.config.get("copy", []):
            source = (self.root / entry).resolve()
            target = self.workdir / source.name
            if source.is_dir():
                shutil.copytree(source, target)
            else:
                shutil.copy2(source, target)
        paths = [str((self.root / p).resolve()) for p in self.config.get("path", [])]
        self.env = dict(os.environ, NO_COLOR="1")
        self.env["PATH"] = os.pathsep.join(paths + [self.env.get("PATH", "")])

    def run(self, name, script):
        self.setup()
        try:
            result = subprocess.run(
                ["bash", "-c", script], cwd=self.workdir, env=self.env,
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                timeout=self.config.get("timeout", 60), text=True)
        except subprocess.TimeoutExpired as e:
            warn(f"{name}: `{script.strip()}` timed out")
            output = e.output.decode() if isinstance(e.output, bytes) else (e.output or "")
            return output + "\n[timed out]\n"
        output = result.stdout
        if result.returncode != 0:
            warn(f"{name}: `{script.strip()}` failed with exit code {result.returncode}")
            output += f"\n[exit code {result.returncode}]\n"
        return output

    def chapter(self, name, content):
        lines = content.splitlines(keepends=True)
        out = []
        i = 0
        while i < len(lines):
            m = FENCE.match(lines[i])
            if not m:
                out.append(lines[i])
                i += 1
                continue
            # Find the closing fence: same character, at least as long.
            fence = m["fence"]
            close = re.compile(rf"^\s*{re.escape(fence[0])}{{{len(fence)},}}\s*$")
            j = i + 1
            while j < len(lines) and not close.match(lines[j]):
                j += 1
            body = lines[i + 1:j]
            closing = lines[j] if j < len(lines) else fence + "\n"
            language = m["info"].split()[0] if m["info"].strip() else ""
            attrs, code = split_header(body)
            classes = attrs.get("classes", [])
            if isinstance(classes, str):
                classes = [classes]

            if "eval" in classes and language in SHELLS:
                output = self.run(name, "".join(code))
                indent = m["indent"]
                out.append(f"{indent}{fence}{language},eval\n")
                out.extend(code)
                out.append(closing)
                out.append("\n")
                out.append(f"{indent}{fence}text,eval-output\n")
                out.extend(f"{indent}{line}\n" for line in output.rstrip("\n").splitlines())
                out.append(f"{indent}{fence}\n")
            else:
                if "eval" in classes:
                    warn(f"{name}: can't evaluate `{language}` blocks")
                out.extend(lines[i:j + 1])
            i = j + 1
        return "".join(out)

    def items(self, items):
        for item in items:
            chapter = item.get("Chapter") if isinstance(item, dict) else None
            if chapter is None:
                continue
            chapter["content"] = self.chapter(chapter.get("name", "?"), chapter["content"])
            self.items(chapter.get("sub_items", []))


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "supports":
        sys.exit(0)

    context, book = json.load(sys.stdin)
    config = context.get("config", {}).get("preprocessor", {}).get("eval", {})
    evaluator = Evaluator(context["root"], config)
    # mdBook 0.5 calls the top-level list `items`; older versions `sections`.
    evaluator.items(book.get("items", book.get("sections", [])))
    json.dump(book, sys.stdout)


if __name__ == "__main__":
    main()
