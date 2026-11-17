#!/usr/bin/env python3
"""mdBook preprocessor that strips YAML front matter from chapters.

Entangled reads a YAML header (delimited by `---` lines) at the top of a
Markdown file; mdBook would render it as a horizontal rule and text, so we
remove it before rendering.
"""

import json
import re
import sys

FRONTMATTER = re.compile(r"\A---[ \t]*\r?\n.*?\r?\n---[ \t]*(\r?\n|\Z)", re.DOTALL)


def strip_items(items):
    for item in items:
        chapter = item.get("Chapter") if isinstance(item, dict) else None
        if chapter is None:
            continue
        chapter["content"] = FRONTMATTER.sub("", chapter["content"], count=1)
        strip_items(chapter.get("sub_items", []))


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "supports":
        sys.exit(0)

    _context, book = json.load(sys.stdin)
    # mdBook 0.5 calls the top-level list `items`; older versions `sections`.
    strip_items(book.get("items", book.get("sections", [])))
    json.dump(book, sys.stdout)


if __name__ == "__main__":
    main()
