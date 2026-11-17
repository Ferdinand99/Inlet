#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Compares the text catalogues under crates/inlet-app/locales with English.

Every language folder next to `en` is checked: texts English has and the language lacks,
texts the language has and English lacks, and texts whose variables differ from English.
Each problem is printed as a GitHub Actions warning annotation (`::warning`) and the
script always exits 0, so a change that adds an English text is not held up while the
translations catch up. The cargo tests of the app fail on texts that English lacks and
on variables that differ (`the_norwegian_catalogue_has_no_text_english_lacks` and
`the_norwegian_texts_take_the_arguments_of_the_english_texts`); `INLET_NB_STRICT=1`
makes `the_norwegian_catalogue_covers_every_english_text` fail too.

Usage: scripts/check-locales.py [locales-dir]
"""

import re
import sys
from pathlib import Path

MESSAGE = re.compile(r"^([A-Za-z][A-Za-z0-9_-]*)\s*=(.*)$")
VARIABLE = re.compile(r"\$([A-Za-z][A-Za-z0-9_-]*)")


def parse(folder: Path) -> dict:
    """Message ID -> (file, line, variables) for every .ftl file of a folder."""
    messages = {}
    for path in sorted(folder.glob("*.ftl")):
        current = None
        lines = path.read_text(encoding="utf-8").split("\n")
        for number, line in enumerate(lines, start=1):
            found = MESSAGE.match(line)
            if found:
                current = found.group(1)
                messages[current] = [path, number, set(VARIABLE.findall(found.group(2)))]
            elif current and (line.startswith((" ", "\t", "}")) and line.strip()):
                messages[current][2].update(VARIABLE.findall(line))
            elif not line.strip() or line.startswith("#"):
                continue
            else:
                current = None
    return messages


def annotate(level: str, path: Path, line: int, text: str) -> None:
    print(f"::{level} file={path.as_posix()},line={line}::{text}")


def main() -> int:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("crates/inlet-app/locales")
    english = parse(root / "en")
    problems = 0
    for folder in sorted(p for p in root.iterdir() if p.is_dir() and p.name != "en"):
        language = parse(folder)
        missing = [i for i in english if i not in language]
        for ident in missing:
            path, line, _ = english[ident]
            annotate(
                "warning",
                path,
                line,
                f"{ident} has no {folder.name} text yet; the English text shows until "
                f"it is added to {folder.as_posix()}",
            )
        for ident, (path, line, variables) in language.items():
            if ident not in english:
                annotate("warning", path, line, f"{ident} is not in locales/en (renamed or removed?)")
            elif variables != english[ident][2]:
                annotate(
                    "warning",
                    path,
                    line,
                    f"{ident} uses {sorted(variables)}, English uses {sorted(english[ident][2])}",
                )
                problems += 1
        extra = [i for i in language if i not in english]
        problems += len(extra)
        total = len(english)
        print(
            f"{folder.name}: {total - len(missing)} of {total} English texts translated, "
            f"{len(missing)} missing, {len(extra)} not in English"
        )
        problems += len(missing)
    print(f"{problems} problem(s); not a failure, see the annotations")
    return 0


if __name__ == "__main__":
    sys.exit(main())
