"""Validate repository-local Markdown link paths and heading fragments."""

from __future__ import annotations

import re
import sys
from pathlib import Path
from urllib.parse import unquote

LINK = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
HEADING = re.compile(r"^#{1,6}\s+(.+?)\s*#*$")
MARKUP = re.compile(r"[`*_~]")
HTML_TAG = re.compile(r"<[^>]+>")
NON_SLUG = re.compile(r"[^\w\- ]", flags=re.UNICODE)
SPACE = re.compile(r"\s+")
EXTERNAL_PREFIXES = ("http://", "https://", "mailto:")


def heading_anchors(markdown: str) -> set[str]:
    """Return GitHub-style anchors, including duplicate-heading suffixes."""
    anchors: set[str] = set()
    counts: dict[str, int] = {}
    for line in markdown.splitlines():
        match = HEADING.match(line)
        if match is None:
            continue
        label = HTML_TAG.sub("", match.group(1))
        label = MARKUP.sub("", label).lower()
        slug = NON_SLUG.sub("", label)
        slug = SPACE.sub("-", slug.strip())
        count = counts.get(slug, 0)
        counts[slug] = count + 1
        anchors.add(slug if count == 0 else f"{slug}-{count}")
    return anchors


def main() -> int:
    """Report every missing repository-local path or Markdown fragment."""
    root = Path(__file__).resolve().parent.parent
    markdown_files = [root / "README.md", *sorted((root / "docs").rglob("*.md"))]
    contents = {path: path.read_text(encoding="utf-8") for path in markdown_files}
    anchors = {path.resolve(): heading_anchors(text) for path, text in contents.items()}
    failures: list[str] = []

    for source, markdown in contents.items():
        for match in LINK.finditer(markdown):
            target = match.group(1).split()[0].strip("<>")
            if target.startswith(EXTERNAL_PREFIXES):
                continue
            path_text, marker, fragment = target.partition("#")
            destination = (source.parent / (path_text or source.name)).resolve()
            line = markdown.count("\n", 0, match.start()) + 1
            location = f"{source.relative_to(root)}:{line}"
            try:
                destination.relative_to(root)
            except ValueError:
                failures.append(f"{location}: local link escapes repository: {target}")
                continue
            if path_text and not destination.exists():
                failures.append(f"{location}: missing local link path: {target}")
            elif (
                marker
                and destination.suffix == ".md"
                and unquote(fragment) not in anchors.get(destination, set())
            ):
                failures.append(f"{location}: missing Markdown heading: {target}")

    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"markdown link targets passed: {len(markdown_files)} files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
