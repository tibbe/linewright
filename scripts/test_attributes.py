"""Run from the repository's root, this makes every test in the `.rs` files
under `crates/` that is not `#[test_log::test]` one, around `#[tokio::test]`
where it is async, and leaves every other byte alone. With `--check`, it
changes no file, prints `path:line: <what to do>` for each such test, in path
then line order, and exits with 1 if it printed any.

A test that is not `#[test_log::test]` is one of two kinds:

- A bare one: a line that is only `#[test]`, `#[tokio::test]` or
  `#[tokio::test(ARGS)]`, reported at that line. The fix makes it
  `#[test_log::test]` or `#[test_log::test(tokio::test(ARGS))]`.
- An rstest one: a `#[rstest]` or `#[rstest::rstest]` attribute in the run of
  attributes, comments and blank lines that ends at a `fn` line, with no
  `#[test_log::test` after it in the run. It is reported at the `#[rstest]`
  line, and the fix writes `#[test_log::test]` on a new line just before the
  `fn`, at its indentation: rstest applies an attribute to every case only when
  it comes after the last `#[case]`
  (https://github.com/la10736/rstest/issues/210). One whose run holds
  `#[test]` or `#[tokio::test` is not one: a bare test is reported and fixed
  once, as that.

What lies in a comment or a string or character literal is not code, so it is
no test. An attribute or a comment that shares its line with what follows it
ends a run with no test, as rustfmt never writes one so. A symlink is not
followed.
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
BARE_ATTRIBUTE = re.compile(r"([ \t]*)#\[(?:test|(tokio::test)(\(.*\))?)\][ \t]*")
RSTEST_ATTRIBUTE = re.compile(r"#\[(?:rstest::)?rstest\]")
FN_LINE = re.compile(
    r"([ \t]*)(?:pub(?:\([^)]*\))?[ \t]+)?(?:const[ \t]+)?(?:async[ \t]+)?"
    r"(?:unsafe[ \t]+)?(?:extern[ \t]+)?fn\b"
)
STRING_LITERAL = re.compile(r'[bc]?r(#*)".*?"\1|[bc]?"(?:[^"\\]|\\.)*"', re.DOTALL)
CHARACTER_LITERAL = re.compile(
    r"b?'(?:[^'\\\n]|\\(?:u\{[0-9a-fA-F_]*\}|x[0-9a-fA-F]{2}|.))'"
)
IDENTIFIER = re.compile(r"[A-Za-z0-9_]+")


@dataclass(frozen=True)
class Bare:
    """A bare test: its attribute is line `line`, 0-based, of its file,
    indented by `indent`. `tokio_args` is `None` for `#[test]`, and for
    `#[tokio::test]` the arguments it is given, with their parentheses, or
    nothing."""

    line: int
    indent: str
    tokio_args: str | None

    def fix(self, lines: list[str]) -> None:
        """Makes its attribute in `lines` the test_log one."""
        if self.tokio_args is None:
            attribute = "#[test_log::test]"
        else:
            attribute = f"#[test_log::test(tokio::test{self.tokio_args})]"
        lines[self.line] = self.indent + attribute


@dataclass(frozen=True)
class Rstest:
    """An rstest test: its `#[rstest]` is line `line`, 0-based, of its file,
    and its `fn` is line `fn_line`, indented by `indent`."""

    line: int
    fn_line: int
    indent: str

    def fix(self, lines: list[str]) -> None:
        """Writes the test_log attribute in `lines` just before its `fn`."""
        lines.insert(self.fn_line, self.indent + "#[test_log::test]")


@dataclass(frozen=True)
class Attribute:
    """An attribute that begins on line `line`, 0-based, of its file, and its
    code, its lines joined."""

    line: int
    code: str


@dataclass(frozen=True)
class Run:
    """The attributes before the `fn` on line `fn_line`, indented by
    `indent`."""

    attributes: list[Attribute]
    fn_line: int
    indent: str

    def rstest(self) -> Rstest | None:
        """It as an rstest test that is not `#[test_log::test]`, if it is one."""
        codes = [attribute.code for attribute in self.attributes]
        rstest = next(
            (n for n, code in enumerate(codes) if RSTEST_ATTRIBUTE.fullmatch(code)),
            None,
        )
        if rstest is None:
            return None
        if any(code == "#[test]" or code.startswith("#[tokio::test") for code in codes):
            return None
        if any(code.startswith("#[test_log::test") for code in codes[rstest + 1 :]):
            return None
        return Rstest(self.attributes[rstest].line, self.fn_line, self.indent)


def code_only(text: str) -> str:
    """`text` with what lies in its comments and literals made spaces, its
    line breaks kept."""
    out = list(text)

    def blank(start: int, end: int) -> None:
        for at in range(start, end):
            if out[at] != "\n":
                out[at] = " "

    at = 0
    while at < len(text):
        if text.startswith("//", at):
            end = text.find("\n", at)
            end = len(text) if end < 0 else end
        elif text.startswith("/*", at):
            end = block_comment_end(text, at)
        elif match := STRING_LITERAL.match(text, at) or CHARACTER_LITERAL.match(
            text, at
        ):
            end = match.end()
        elif match := IDENTIFIER.match(text, at):
            at = match.end()
            continue
        else:
            at += 1
            continue
        blank(at, end)
        at = end
    return "".join(out)


def block_comment_end(text: str, start: int) -> int:
    """Where the block comment at `start` of `text` ends, the nested ones in
    it counted, or the end of `text` if it does not."""
    depth = 0
    at = start
    while at < len(text):
        if text.startswith("/*", at):
            depth += 1
            at += 2
        elif text.startswith("*/", at):
            depth -= 1
            at += 2
            if depth == 0:
                return at
        else:
            at += 1
    return len(text)


def attribute_at(code: list[str], start: int) -> tuple[Attribute, int] | None:
    """The attribute that begins line `start` of `code` and the line after it,
    if nothing follows it on its last line."""
    depth = 0
    for number in range(start, len(code)):
        for at, char in enumerate(code[number]):
            if number == start and at < code[number].index("#"):
                continue
            if char in "[({":
                depth += 1
            elif char in "])}":
                depth -= 1
                if depth == 0:
                    if code[number][at + 1 :].strip():
                        return None
                    text = "\n".join(code[start:number] + [code[number][: at + 1]])
                    return Attribute(start, text.strip()), number + 1
    return None


def run_at(code: list[str], start: int) -> Run | None:
    """The run of attributes and blank lines from line `start` of `code` to a
    `fn` line, if it ends at one."""
    attributes = []
    number = start
    while number < len(code):
        stripped = code[number].strip()
        if not stripped:
            number += 1
        elif stripped.startswith("#["):
            found = attribute_at(code, number)
            if found is None:
                return None
            attribute, number = found
            attributes.append(attribute)
        elif match := FN_LINE.match(code[number]):
            return Run(attributes, number, match.group(1))
        else:
            return None
    return None


def findings(lines: list[str]) -> list[Bare | Rstest]:
    """The tests in a file of `lines`, without their `\\n`, that are not
    `#[test_log::test]`, in line order."""
    code = code_only("\n".join(lines)).split("\n")
    found: list[Bare | Rstest] = []
    for number, line in enumerate(lines):
        match = BARE_ATTRIBUTE.fullmatch(line)
        if match is not None and code[number].lstrip().startswith("#["):
            indent, tokio, args = match.groups()
            tokio_args = None if tokio is None else (args or "")
            found.append(Bare(number, indent, tokio_args))
    number = 0
    while number < len(code):
        run = run_at(code, number) if code[number].lstrip().startswith("#[") else None
        if run is None:
            number += 1
            continue
        if (rstest := run.rstest()) is not None:
            found.append(rstest)
        number = run.fn_line + 1
    return sorted(found, key=lambda finding: finding.line)


def fixed(lines: list[str], found: list[Bare | Rstest]) -> list[str]:
    """`lines` with each of `found` in them made `#[test_log::test]`."""
    lines = list(lines)
    for finding in sorted(found, key=lambda finding: finding.line, reverse=True):
        finding.fix(lines)
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
            lines = file.read().decode("utf-8", "surrogateescape").split("\n")
        found = findings(lines)
        if check:
            for finding in found:
                print(f"{path}:{finding.line + 1}: {REPORT}")
            reported = reported or bool(found)
        elif found:
            text = "\n".join(fixed(lines, found))
            write(path, text.encode("utf-8", "surrogateescape"))
    return 1 if reported else 0


if __name__ == "__main__":
    sys.exit(main())
