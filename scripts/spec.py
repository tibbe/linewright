"""Reading the spec and its model: the spec's sections, tables and
sentences without their Markdown, and the model's comments and spec-table
directives (`scripts/model_tables.py` describes them)."""

import glob, re
from collections import namedtuple

SPEC_FILES = sorted(glob.glob("docs/specs/**/*.md", recursive=True)) + ["GLOSSARY.md"]
MODEL_FILES = sorted(glob.glob("docs/model/*.qnt"))
STATES_TABLE = ("docs/specs/design_system.md", "## States")
COMMENT = re.compile(r"\s*(?:///?|/\*\*|\*(?!/))\s?(.*)")
QUOTE = re.compile(r'"([^"]+)"')
ELLIPSIS = re.compile(r"\s*(?:\.\.\.|…)\s*")
LETTER = re.compile(r"\b([A-Z]{1,2}) = (\w+\.md)\b")
SENTENCE_END = re.compile(r"(?<=[.!?:])\s+(?=[A-Z\"'(])")
DIRECTIVE = re.compile(r'^(\s*)// spec-table (\w+) "([^"]+)"(?: (\d+))? (\w+)((?: \w+=\w+)*)\s*$')
DIRECTIVE_END = re.compile(r"^\s*// spec-table end\s*$")
HEADER = re.compile(r"^\s*pure (?:def|val) (\w+)(?:\(([^)]*)\))?: .*=\s*$")

Run = namedtuple("Run", "path line text starts")
Section = namedtuple("Section", "path heading lines")
Table = namedtuple("Table", "path line header rows")
Row = namedtuple("Row", "line cells")
Directive = namedtuple("Directive", "path line indent letter heading ordinal kind options name args body_start body_end")


def plain(s):
    """`s` as the spec reads without its Markdown: links and code spans in
    parentheses dropped, other links reduced to their text, and `**`, `_` and
    backticks removed."""
    s = re.sub(r"\s+", " ", s)
    s = re.sub(r" \((?:\[[^\]]*\]\([^)]*\)|`[^`]*`)[^()]*\)", "", s)
    s = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", s)
    return re.sub(r"\s+", " ", s.replace("`", "").replace("**", "").replace("_", " ")).strip()


def unquoted(s):
    return plain(s).replace('"', "").strip()


def slug(s):
    return re.sub(r"[^a-z0-9]+", "_", unquoted(s).lower()).strip("_")


def literal(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def comment_runs(path):
    """The runs of consecutive comment lines in a model file, each joined into
    one text, with the offset at which each line starts."""
    runs, lines = [], []

    def close():
        if lines:
            text, starts = "", []
            for number, line in lines:
                starts.append((len(text), number))
                text += line + " "
            runs.append(Run(path, lines[0][0], text, starts))
            lines.clear()

    for number, line in enumerate(open(path), 1):
        m = COMMENT.match(line)
        if m:
            lines.append((number, m.group(1)))
        else:
            close()
    close()
    return runs


def line_of(run, offset):
    return max(number for at, number in run.starts if at <= offset)


def quote_parts(quote):
    return [p for p in (plain(p) for p in ELLIPSIS.split(quote)) if p]


def sections(path):
    """Each heading of a spec file, with its lines up to the next heading of
    its level or above."""
    lines = open(path).read().split("\n")
    heads = [
        (i, len(m.group(1)), m.group(1) + " " + m.group(2).strip())
        for i, line in enumerate(lines)
        if (m := re.match(r"(#+) (.*)", line))
    ]
    out = {}
    for k, (i, level, heading) in enumerate(heads):
        end = next((j for j, lv, _ in heads[k + 1 :] if lv <= level), len(lines))
        out[heading] = Section(path, heading, [(n + 1, lines[n]) for n in range(i + 1, end)])
    return out


def tables_in(section):
    """The tables of a section, each row's cells as the spec reads them
    without Markdown."""
    tables, table = [], None
    for number, line in section.lines + [(0, "")]:
        if line.startswith("|"):
            cells = [plain(c) for c in line.strip().strip("|").split("|")]
            if all(re.fullmatch(r":?-+:?", c) for c in cells):
                continue
            if table is None:
                table = Table(section.path, number, cells, [])
            else:
                table.rows.append(Row(number, cells))
        elif table is not None:
            tables.append(table)
            table = None
    return tables


def sentences_in(section):
    """A section's sentences, its headings, tables, code blocks and bullet
    labels aside, each with the line it starts on."""
    out, paragraph, fenced = [], [], False

    def close():
        if paragraph:
            text = plain(" ".join(t for _, t in paragraph))
            for s in SENTENCE_END.split(text):
                head = " ".join(s.split()[:4])
                line = next((n for n, t in paragraph if head in plain(t)), paragraph[0][0])
                out.append((line, s))
            paragraph.clear()

    for number, line in section.lines + [(0, "")]:
        if line.startswith("```"):
            fenced = not fenced
            continue
        if fenced or re.match(r"#+ ", line) or line.startswith("|"):
            close()
            continue
        bullet = re.match(r"\s*(?:- |\d+\. )(?:\*\*[^*]+\*\*\s*)?(.*)", line)
        if not line.strip() or bullet:
            close()
        text = bullet.group(1) if bullet else line
        if text.strip():
            paragraph.append((number, text))
    close()
    return out


def letters_of(path):
    text = " ".join(r.text for r in comment_runs(path))
    return {a: "docs/specs/" + f for a, f in LETTER.findall(text)}


def known_states():
    section = sections(STATES_TABLE[0])[STATES_TABLE[1]]
    return {unquoted(r.cells[0]) for r in tables_in(section)[0].rows}


def directives(path):
    lines = open(path).read().split("\n")
    out = []
    for k, line in enumerate(lines):
        m = DIRECTIVE.match(line)
        if not m:
            continue
        indent, letter, heading, ordinal, kind, options = m.groups()
        header = HEADER.match(lines[k + 1]) if k + 1 < len(lines) else None
        if not header:
            raise Finding(f"{path}:{k + 2}: a spec-table directive must be followed by `pure def NAME(...): T =` or `pure val NAME: T =`")
        end = next((j for j in range(k + 2, len(lines)) if DIRECTIVE_END.match(lines[j])), None)
        if end is None:
            raise Finding(f"{path}:{k + 1}: spec-table directive without `// spec-table end`")
        args = [p.split(":")[0].strip() for p in (header.group(2) or "").split(",") if p.strip()]
        opts = dict(o.split("=") for o in options.split())
        out.append(Directive(path, k + 1, indent, letter, heading, int(ordinal or 1), kind, opts,
                             header.group(1), args, k + 2, end))
    return out


class Finding(Exception):
    pass
