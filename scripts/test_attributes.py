"""Run from the repository's root, this makes every test in the `.rs` files
under `crates/` that is not `#[test_log::test]` one, around `#[tokio::test]`
where it is async, and leaves every other byte alone. With `--check`, it
changes no file, prints `path:line: <what to do>` for each such test, in path
then line order, and exits with 1 if it printed any.

A test that is not `#[test_log::test]` is a bare one: a line that is only
`#[test]`, `#[tokio::test]` or `#[tokio::test(ARGS)]`, reported at that line.
The fix makes it `#[test_log::test]` or `#[test_log::test(tokio::test(ARGS))]`.
A symlink is not followed.
"""

import os
import re
import sys
import tempfile
from dataclasses import dataclass

ROOT = "crates"
REPORT = (
    "use `#[test_log::test]`, around `#[tokio::test]` where async: "
    "run `scripts/fix test`"
)
BARE_ATTRIBUTE = re.compile(
    rb"([ \t]*)#\[(?:test|(tokio::test)(\(.*\))?)\][ \t]*", re.DOTALL
)


@dataclass(frozen=True)
class Bare:
    """A bare test: its attribute is line `line`, 0-based, of its file,
    indented by `indent`. `tokio_args` is `None` for `#[test]`, and for
    `#[tokio::test]` the arguments it is given, with their parentheses, or
    nothing."""

    line: int
    indent: bytes
    tokio_args: bytes | None

    def attribute(self) -> bytes:
        """The test_log attribute that takes its place, indented."""
        if self.tokio_args is None:
            return self.indent + b"#[test_log::test]"
        return self.indent + b"#[test_log::test(tokio::test" + self.tokio_args + b")]"


def findings(lines: list[bytes]) -> list[Bare]:
    """The tests in a file of `lines`, without their `\\n`, that are not
    `#[test_log::test]`, in line order."""
    found = []
    for number, line in enumerate(lines):
        match = BARE_ATTRIBUTE.fullmatch(line)
        if match is None:
            continue
        indent, tokio, args = match.groups()
        tokio_args = None if tokio is None else (args or b"")
        found.append(Bare(number, indent, tokio_args))
    return found


def fixed(lines: list[bytes], found: list[Bare]) -> list[bytes]:
    """`lines` with each of `found` in them made `#[test_log::test]`."""
    lines = list(lines)
    for finding in found:
        lines[finding.line] = finding.attribute()
    return lines


def sources() -> list[str]:
    """The `.rs` files under `crates/` that are not symlinks, in path order."""
    paths = []
    for directory, _, files in os.walk(ROOT):
        for name in files:
            path = os.path.join(directory, name)
            if name.endswith(".rs") and not os.path.islink(path):
                paths.append(path)
    return sorted(paths, key=lambda path: path.split(os.sep))


def write(path: str, content: bytes) -> None:
    """Replaces the file at `path` with `content` at once, keeping its mode."""
    directory = os.path.dirname(path)
    with tempfile.NamedTemporaryFile(dir=directory, delete=False) as file:
        file.write(content)
    os.chmod(file.name, os.stat(path).st_mode)
    os.replace(file.name, path)


def main() -> int:
    if sys.argv[1:] not in ([], ["--check"]):
        sys.exit(f"usage: {sys.argv[0]} [--check]")
    check = sys.argv[1:] == ["--check"]
    reported = False
    for path in sources():
        with open(path, "rb") as file:
            lines = file.read().split(b"\n")
        found = findings(lines)
        if check:
            for finding in found:
                print(f"{path}:{finding.line + 1}: {REPORT}")
            reported = reported or bool(found)
        elif found:
            write(path, b"\n".join(fixed(lines, found)))
    return 1 if reported else 0


if __name__ == "__main__":
    sys.exit(main())
