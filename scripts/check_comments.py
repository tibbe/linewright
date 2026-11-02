"""`scripts/check comments`: prints `path:line: <why>` for every comment block
in the tracked files that is not one of the four kinds CODING_STANDARDS.md
allows, and exits with 1 if it printed anything."""

import os, re, subprocess, sys
from collections import namedtuple

VENDORED_OR_MODEL = re.compile(
    r"^docs/model/|^assets/htmx\.min\.js$|^assets/hx-sse\.js$|^styles/daisyui(-theme)?\.mjs$"
)
LINK = re.compile(r"https?://")
LICENSE = re.compile(r"@license|SPDX-License-Identifier")
SHELLCHECK = re.compile(r"shellcheck disable=")
SHELLCHECK_REASON = re.compile(r"shellcheck disable=\S+\s+#\s*\S")
NOT_ALLOWED = "not a contract, a linked outside constraint, a directive or a license notice"


class Comment:
    def __init__(self, line, end, text, doc=False, trailing=False):
        self.line, self.end, self.text = line, end, text
        self.doc, self.trailing = doc, trailing


def line_at(src, i):
    return src.count("\n", 0, i) + 1


def find_or_end(src, needle, i):
    j = src.find(needle, i)
    return len(src) if j < 0 else j


def after_code(src, i):
    start = src.rfind("\n", 0, i) + 1
    return src[start:i].strip() != ""


def is_doc_block(src, i):
    return src.startswith("/**", i) and not re.match(r"/\*\*[*/]", src[i : i + 4])


def skip_quoted(src, i, quote, escapes=True):
    i += 1
    while i < len(src):
        if escapes and src[i] == "\\":
            i += 2
        elif src[i] == quote:
            return i + 1
        else:
            i += 1
    return i


def block_comment(src, i, comments, doc, nests):
    depth, j = 1, i + 2
    while j < len(src) and depth:
        if nests and src.startswith("/*", j):
            depth, j = depth + 1, j + 2
        elif src.startswith("*/", j):
            depth, j = depth - 1, j + 2
        else:
            j += 1
    comments.append(Comment(line_at(src, i), line_at(src, j - 1), src[i + 2 : j - 2],
                            doc, after_code(src, i)))
    return j


def line_comment(src, i, comments, start, doc=False):
    j = find_or_end(src, "\n", i)
    comments.append(Comment(line_at(src, i), line_at(src, i), src[i + start : j],
                            doc, after_code(src, i)))
    return j


def lex_rust(src):
    comments, i = [], 0
    char = re.compile(r"'(\\(u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'")
    raw = re.compile(r"[bc]?r(#*)\"")
    while i < len(src):
        c = src[i]
        if src.startswith("//", i):
            doc = (src.startswith("///", i) and not src.startswith("////", i)) or src.startswith("//!", i)
            i = line_comment(src, i, comments, 2, doc)
        elif src.startswith("/*", i):
            doc = is_doc_block(src, i) or src.startswith("/*!", i)
            i = block_comment(src, i, comments, doc, nests=True)
        elif (m := raw.match(src, i)) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            close = '"' + m.group(1)
            i = find_or_end(src, close, m.end()) + len(close)
        elif c == '"':
            i = skip_quoted(src, i, '"')
        elif c == "'":
            m = char.match(src, i)
            i = m.end() if m else i + 1
        else:
            i += 1
    return comments


def lex_js(src):
    comments, i, braces = [], 0, []
    after_operand = False
    while i < len(src):
        c = src[i]
        if src.startswith("//", i):
            i = line_comment(src, i, comments, 2)
        elif src.startswith("/*", i):
            i = block_comment(src, i, comments, is_doc_block(src, i), nests=False)
        elif c in "\"'":
            i, after_operand = skip_quoted(src, i, c), True
        elif c == "`" or (c == "}" and braces and braces[-1] == "`"):
            if c == "}":
                braces.pop()
            i += 1
            while i < len(src) and src[i] != "`":
                if src[i] == "\\":
                    i += 2
                elif src.startswith("${", i):
                    braces.append("`")
                    i += 2
                    break
                else:
                    i += 1
            else:
                i, after_operand = i + 1, True
        elif c == "/" and not (after_operand and not re.search(r"\b(return|typeof|case|in|of)\s*$", src[max(0, i - 8) : i])):
            in_class, i = False, i + 1
            while i < len(src) and src[i] != "\n":
                if src[i] == "\\":
                    i += 1
                elif src[i] == "[":
                    in_class = True
                elif src[i] == "]":
                    in_class = False
                elif src[i] == "/" and not in_class:
                    break
                i += 1
            i, after_operand = i + 1, True
        else:
            if c == "{":
                braces.append("{")
            elif c == "}" and braces:
                braces.pop()
            if not c.isspace():
                after_operand = c.isalnum() or c in "_$)]"
            i += 1
    return comments


def lex_css(src):
    comments, i = [], 0
    while i < len(src):
        if src.startswith("/*", i):
            i = block_comment(src, i, comments, False, nests=False)
        elif src[i] in "\"'":
            i = skip_quoted(src, i, src[i])
        else:
            i += 1
    return comments


def lex_toml(src):
    comments, i = [], 0
    while i < len(src):
        if src.startswith('"""', i) or src.startswith("'''", i):
            quote = src[i : i + 3]
            i = find_or_end(src, quote, i + 3) + 3
            while i < len(src) and src[i] == quote[0]:
                i += 1
        elif src[i] in "\"'":
            i = skip_quoted(src, i, src[i], escapes=src[i] == '"')
        elif src[i] == "#":
            i = line_comment(src, i, comments, 1)
        else:
            i += 1
    return comments


def lex_yaml(src):
    comments, lines = [], src.split("\n")
    block_indent, quote = None, None
    for n, text in enumerate(lines, 1):
        indent = len(text) - len(text.lstrip(" "))
        if block_indent is not None:
            if text.strip() == "" or indent > block_indent:
                continue
            block_indent = None
        i, end, scalar_start = 0, len(text), True
        while i < len(text):
            c = text[i]
            if quote:
                if quote == '"' and c == "\\":
                    i += 2
                    continue
                if c == quote:
                    if quote == "'" and text.startswith("''", i):
                        i += 2
                        continue
                    quote = None
                i += 1
                continue
            if c == "#" and (i == 0 or text[i - 1] in " \t"):
                comments.append(Comment(n, n, text[i + 1 :], False, text[:i].strip() != ""))
                end = i
                break
            if c in "\"'" and scalar_start:
                quote = c
            elif not c.isspace():
                scalar_start = c in ":-[{,?" and (i + 1 == len(text) or text[i + 1] in " \t" or c in "[{,")
            i += 1
        if not quote and re.search(r"(^|[:-])\s*[|>][-+0-9]*$", text[:end].strip()):
            block_indent = indent
    return comments


def lex_hash_lines(src):
    return [Comment(n, n, line.lstrip()[1:], False, False)
            for n, line in enumerate(src.split("\n"), 1) if line.lstrip().startswith("#")]


def heredoc_bodies(src):
    inside, delimiter, strip = set(), None, False
    for n, line in enumerate(src.split("\n"), 1):
        if delimiter is not None:
            if (line.lstrip("\t") if strip else line) == delimiter:
                delimiter = None
            else:
                inside.add(n)
            continue
        m = re.search(r"<<(-?)\s*(['\"]?)([\w.-]+)\2", line)
        if m and not line[: m.start()].endswith("<"):
            delimiter, strip = m.group(3), m.group(1) == "-"
    return inside


def lex_dockerfile(src):
    inside = heredoc_bodies(src)
    return [c for c in lex_hash_lines(src) if c.line not in inside]


CODE, DOUBLE_QUOTED = "code", "double-quoted"


def lex_shell(src):
    comments, inside = [], heredoc_bodies(src)
    modes, i, line = [CODE], 0, 1
    while i < len(src):
        c = src[i]
        if c == "\n":
            line += 1
            i += 1
            while line in inside and i < len(src):
                i, line = find_or_end(src, "\n", i) + 1, line + 1
            continue
        if c == "\\":
            line += src[i + 1 : i + 2] == "\n"
            i += 2
            continue
        if modes[-1] == DOUBLE_QUOTED:
            if c == '"':
                modes.pop()
            elif src.startswith("$(", i):
                modes.append(CODE)
                i += 1
            i += 1
            continue
        if c == "#" and (i == 0 or src[i - 1] in " \t\n;&|()<>"):
            j = find_or_end(src, "\n", i)
            comments.append(Comment(line, line, src[i + 1 : j], False, after_code(src, i)))
            i = j
        elif src.startswith("$'", i):
            i = skip_quoted(src, i + 1, "'")
        elif c == "'":
            j = skip_quoted(src, i, "'", escapes=False)
            line += src.count("\n", i, j)
            i = j
        elif c == '"':
            modes.append(DOUBLE_QUOTED)
            i += 1
        elif src.startswith("$(", i):
            modes.append(CODE)
            i += 2
        elif c == ")" and len(modes) > 1:
            modes.pop()
            i += 1
        else:
            i += 1
    return comments


def lex_python(src):
    comments, i = [], 0
    while i < len(src):
        if src.startswith('"""', i) or src.startswith("'''", i):
            quote, i = src[i : i + 3], i + 3
            while i < len(src) and not src.startswith(quote, i):
                i += 2 if src[i] == "\\" else 1
            i += 3
        elif src[i] in "\"'":
            i = skip_quoted(src, i, src[i])
        elif src[i] == "#":
            i = line_comment(src, i, comments, 1)
        else:
            i += 1
    return comments


def lex_template(src):
    comments, i = [], 0
    while i < len(src):
        if src.startswith("{#", i):
            depth, j = 0, i
            while j < len(src):
                if src.startswith("{#", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("#}", j):
                    depth, j = depth - 1, j + 2
                    if depth == 0:
                        break
                else:
                    j += 1
            text = src[i + 2 : j - 2].strip("-+~")
            comments.append(Comment(line_at(src, i), line_at(src, j - 1), text, False, after_code(src, i)))
            i = j
        elif src.startswith("<!--", i):
            j = find_or_end(src, "-->", i) + 3
            comments.append(Comment(line_at(src, i), line_at(src, j - 1), src[i + 4 : j - 3],
                                    False, after_code(src, i)))
            i = j
        elif src.startswith("{{", i) or src.startswith("{%", i):
            close = "}}" if src[i + 1] == "{" else "%}"
            j = i + 2
            while j < len(src) and not src.startswith(close, j):
                j = skip_quoted(src, j, src[j]) if src[j] in "\"'" else j + 1
            i = j + 2
        else:
            i += 1
    return comments


class Language(namedtuple("Language", "lex first_line documents", defaults=(None, None))):
    """A language's lexer, the directive its first line may hold, and the
    definition a comment block directly above it documents, where the language
    has no doc comments."""


SHELL = Language(lex_shell, re.compile(r"!"),
                 re.compile(r"\s*(function\s+)?[\w-]+\s*\(\s*\)\s*\{?\s*$|\s*function\s+[\w-]+\s*\{?\s*$"))
PYTHON = Language(lex_python, re.compile(r"!"))
DOCKERFILE = Language(lex_dockerfile, re.compile(r"\s*syntax="))
HASH_LINES = Language(lex_hash_lines)
BY_EXTENSION = {
    ".rs": Language(lex_rust), ".js": Language(lex_js), ".mjs": Language(lex_js),
    ".py": PYTHON, ".css": Language(lex_css), ".toml": Language(lex_toml),
    ".yml": Language(lex_yaml), ".yaml": Language(lex_yaml),
    ".html": Language(lex_template, None, re.compile(r"\s*\{%[-+~]?\s*macro\s")),
}


def language(path, src):
    name = os.path.basename(path)
    if re.match(r"#!.*\b(ba|z)?sh\b", src):
        return SHELL
    if re.match(r"#!.*\bpython", src):
        return PYTHON
    if name == "Dockerfile":
        return DOCKERFILE
    if name in (".gitignore", ".dockerignore", "CODEOWNERS") or name.endswith(".env.example"):
        return HASH_LINES
    return BY_EXTENSION.get(os.path.splitext(name)[1])


def directive(lang, comment):
    if comment.line == 1 and lang.first_line and lang.first_line.match(comment.text):
        return True
    return lang is SHELL and SHELLCHECK_REASON.search(comment.text) is not None


def violations(path, lang, src, comments):
    lines = src.split("\n")
    found, block = [], []

    def close():
        if not block:
            return
        text = "\n".join(c.text for c in block)
        after = lines[block[-1].end] if block[-1].end < len(lines) else ""
        documents = lang.documents and not block[0].trailing and lang.documents.match(after)
        if not (LINK.search(text) or LICENSE.search(text) or documents):
            found.append(f"{path}:{block[0].line}: {NOT_ALLOWED}")
        block.clear()

    for comment in comments:
        if comment.doc or directive(lang, comment):
            close()
            continue
        if lang is SHELL and SHELLCHECK.search(comment.text):
            close()
            found.append(f"{path}:{comment.line}: a `# shellcheck disable=` needs a `# reason` after it")
            continue
        if comment.trailing or not block or block[-1].trailing or comment.line != block[-1].end + 1:
            close()
        block.append(comment)
        if comment.trailing:
            close()
    close()
    return found


def outcome(path, src):
    """What the check reports for the file at `path` holding `src`."""
    lang = language(path, src)
    if VENDORED_OR_MODEL.search(path) or lang is None:
        return []
    return violations(path, lang, src, lang.lex(src))


def main():
    files = subprocess.run(["git", "ls-files", "-z"], capture_output=True, text=True, check=True).stdout
    reports = []
    for path in sorted(p for p in files.split("\0") if p):
        if not os.path.islink(path) and os.path.isfile(path):
            with open(path, encoding="utf-8", errors="replace") as file:
                reports += outcome(path, file.read())
    print("\n".join(reports), end="\n" if reports else "")
    sys.exit(1 if reports else 0)


if __name__ == "__main__":
    main()
