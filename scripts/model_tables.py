"""`scripts/fix model-tables` runs this bare: it writes the definitions the
model's spec-table directives generate from the spec's tables. `scripts/check
model-tables` runs it with `--check`: it prints `path:line: <why>` where one
does not read as the spec's table does now, and exits with 1 if it printed
any. Every `when_`, `does_` and `result_` definition they call exists in
their file, and every such definition is called by one, so that a row added
to, removed from or reworded in the spec fails the check until the model
follows it.

A directive is the comment
`// spec-table L "## Heading" [N] KIND [OPTION=NAME...]` on the line before
a definition's header, whose body runs to the line `// spec-table end`. L is
a spec file's letter, given in the file's header as `R = repository.md`; N
picks the section's Nth table, the first by default.
The kinds, and what the model gives each, with `A` the header's arguments:

- `state`: a first-match table of (when | state), as a chain of ifs. A row's
  state is a literal where it is one of the states in design_system.md's
  States table or "no state" (""); otherwise `result_<when>(A)` gives it.
  Its when is `when_<when>(A)`, or `else` for "otherwise".
- `index`: the same table, as the number, from 1, of the first row that holds.
- `rows`: a first-match table of (state | when | Linewright | state after), as
  a chain of ifs over `inState(A, Set(...))` for its state, or its negation
  for "any but ...", and `when_<when>(A)`, giving `does_<Linewright>(A)`.
  With `keep=K`, a row holds only where `K(A, does_<Linewright>(A))` also
  does. With `else=E`, `E(A)` is what it gives where no row holds; a table
  whose rows can all fail to hold needs one.
- `row`: the same table, as the number, from 1, of the first row that holds.
- `lines`: a first-match table of (column | line | when | status), or of
  (line | when | status) without `column=`, as a chain of ifs over
  `P == "<column>" and when_<when>(A)`. The line is the first quoted string
  of its cell, or `result_<line>(A)` where the cell quotes none. A row that
  reads "as in <column>" is that column's rows. A comes without P.
- `map`: a table of (key | value) as a Map of its literals.
- `board`: the board's table of (column | an issue's states | a pull request's
  states) as a list of (column, Set, Set).

`<when>` is the cell's text in lower case with each run of other characters
as `_`, its code and links reduced to their text."""

import os, re, sys

# -I leaves the script's directory off the path:
# https://docs.python.org/3/using/cmdline.html#cmdoption-I
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from spec import (
    Finding,
    MODEL_FILES,
    QUOTE,
    directives,
    known_states,
    letters_of,
    literal,
    sections,
    slug,
    tables_in,
    unquoted,
)

DEFINITION = re.compile(r"^\s*pure def ((?:when|does|result)_\w+)\(", re.M)
AS_IN = re.compile(r"as in (.+)")


def state_set(cell):
    """The states a row's state cell names, "no state" as ""."""
    return ["" if s == "no state" else s for s in re.split(r",\s*|\s+and\s+|\s+or\s+", unquoted(cell)) if s]


def state_condition(cell, args):
    text = unquoted(cell)
    if text == "any":
        return None
    if text.startswith("any but "):
        return f"not(inState({args}, Set({', '.join(literal(s) for s in state_set(text[len('any but '):]))})))"
    return f"inState({args}, Set({', '.join(literal(s) for s in state_set(text))}))"


def otherwise(cell):
    return re.match(r"otherwise\b", unquoted(cell)) is not None


def when_call(cell, args, calls, row):
    name = f"when_{slug(cell)}"
    calls.append((name, row, cell))
    return f"{name}({args})"


def generate(d, table, states, calls):
    """The body a directive generates, unindented, and the callees it names."""
    args = ", ".join(a for a in d.args if a != d.options.get("column"))
    rows = [(" | ".join(r.cells), r) for r in table.rows]
    out = []
    if d.kind in ("state", "index"):
        for k, (text, row) in enumerate(rows):
            when, state = unquoted(row.cells[0]), unquoted(row.cells[1])
            if d.kind == "index":
                result = str(k + 1)
            elif state in states or state == "no state":
                result = literal("" if state == "no state" else state)
            else:
                name = f"result_{slug(state)}"
                calls.append((name, row, row.cells[1]))
                result = f"{name}({args})"
            out.append(f"// {text}")
            if otherwise(when):
                out.append(f"else {result}")
            else:
                out.append(f"{'if' if k == 0 else 'else if'} ({when_call(row.cells[0], args, calls, row)}) {result}")
        if not otherwise(rows[-1][1].cells[0]):
            out.append('else ""' if d.kind == "state" else "else 0")
    elif d.kind in ("rows", "row"):
        keep, fallback = d.options.get("keep"), d.options.get("else")
        unconditional = False
        for k, (text, row) in enumerate(rows):
            if unconditional:
                raise Finding(f"{d.path}:{d.line}: no row after {table.path}:{rows[k - 1][1].line} can hold")
            state, when, does = row.cells[0], row.cells[1], row.cells[2]
            parts = [state_condition(state, args)]
            if not otherwise(when):
                parts.append(when_call(when, args, calls, row))
            if d.kind == "row":
                result = str(k + 1)
            else:
                name = f"does_{slug(does)}"
                calls.append((name, row, does))
                result = f"{name}({args})"
                if keep:
                    parts.append(f"{keep}({args}, {result})")
            condition = " and ".join(p for p in parts if p)
            out.append(f"// {text}")
            if not condition:
                out.append(f"else {result}" if k else result)
                unconditional = True
            else:
                out.append(f"{'if' if k == 0 else 'else if'} ({condition}) {result}")
        if not unconditional:
            if d.kind == "row":
                out.append("else 0")
            elif fallback:
                out.append(f"else {fallback}({args})")
            else:
                raise Finding(f"{d.path}:{d.line}: a row can fail to hold, so the directive needs else=NAME")
    elif d.kind == "lines":
        column = d.options.get("column")
        groups, current = [], None
        for text, row in rows:
            cells = list(row.cells)
            if column:
                current = cells[0] or current
                cells[0] = current
            groups.append((text, row, cells))
        expanded = []
        for text, row, cells in groups:
            line = cells[1] if column else cells[0]
            m = AS_IN.fullmatch(unquoted(line))
            if m and column:
                expanded += [(r, [cells[0]] + c[1:]) for t, r, c in groups if unquoted(c[0]) == m.group(1)]
            else:
                expanded.append((row, cells))
        first = True
        for row, cells in expanded:
            text = " | ".join(cells)
            line, when = (cells[1], cells[2]) if column else (cells[0], cells[1])
            parts = [f"{column} == {literal(unquoted(cells[0]))}"] if column else []
            if not otherwise(when):
                parts.append(when_call(when, args, calls, row))
            quoted = QUOTE.search(line)
            if quoted:
                result = literal(quoted.group(1))
            else:
                name = f"result_{slug(line)}"
                calls.append((name, row, line))
                result = f"{name}({args})"
            out.append(f"// {text}")
            if parts:
                out.append(f"{'if' if first else 'else if'} ({' and '.join(parts)}) {result}")
            else:
                out.append(f"else {result}")
            first = False
        if parts:
            out.append('else ""')
    elif d.kind == "map":
        out.append("Map(")
        for text, row in rows:
            out.append(f"  {literal(unquoted(row.cells[0]))} -> {literal(unquoted(row.cells[1]))},")
        out.append(")")
    elif d.kind == "board":
        out.append("[")
        for text, row in rows:
            sets = ", ".join(f"Set({', '.join(literal(s) for s in state_set(c))})" for c in row.cells[1:])
            out.append(f"  ({literal(unquoted(row.cells[0]))}, {sets}),")
        out.append("]")
    else:
        raise Finding(f"{d.path}:{d.line}: no spec-table kind {d.kind}")
    return out


def tables(fix):
    findings = []
    states = known_states()
    spec_cache = {}
    for path in MODEL_FILES:
        letters = letters_of(path)
        lines = open(path).read().split("\n")
        text = "\n".join(lines)
        try:
            ds = directives(path)
        except Finding as f:
            findings.append(str(f))
            continue
        calls, edits = [], []
        for d in ds:
            if d.letter not in letters:
                findings.append(f"{path}:{d.line}: {d.letter} names no spec file in this file's header")
                continue
            spec_path = letters[d.letter]
            if spec_path not in spec_cache:
                spec_cache[spec_path] = sections(spec_path)
            if d.heading not in spec_cache[spec_path]:
                findings.append(f"{path}:{d.line}: {spec_path} has no heading {d.heading}")
                continue
            tables = tables_in(spec_cache[spec_path][d.heading])
            if len(tables) < d.ordinal:
                findings.append(f"{path}:{d.line}: {spec_path} {d.heading} has {len(tables)} tables, not {d.ordinal}")
                continue
            table = tables[d.ordinal - 1]
            row_calls = []
            try:
                body = generate(d, table, states, row_calls)
            except Finding as f:
                findings.append(str(f))
                continue
            callee_args = ", ".join(a for a in d.args if a != d.options.get("column"))
            calls += [(d, spec_path, callee_args, c) for c in row_calls]
            body = [d.indent + "  " + b if b else b for b in body]
            if lines[d.body_start : d.body_end] != body:
                if fix:
                    edits.append((d.body_start, d.body_end, body))
                else:
                    findings.append(f"{path}:{d.line}: {d.name} does not read as {spec_path}:{table.line} does: run `scripts/fix model-tables` and commit it")
        for start, end, body in sorted(edits, reverse=True):
            lines[start:end] = body
        if edits:
            open(path, "w").write("\n".join(lines))
            text = "\n".join(lines)
        defined = {m.group(1): text.count("\n", 0, m.start()) + 1 for m in DEFINITION.finditer(text)}
        called = set()
        for d, spec_path, args, (name, row, cell) in calls:
            called.add(name)
            finding = f'{path}:{d.line}: {d.name} needs `pure def {name}({args})` for "{unquoted(cell)}", the row at {spec_path}:{row.line}'
            if name not in defined and finding not in findings:
                findings.append(finding)
        for name, line in sorted(defined.items(), key=lambda kv: kv[1]):
            if name not in called:
                findings.append(f"{path}:{line}: {name} encodes no cell of the spec's tables")
    return findings


if __name__ == "__main__":
    args = sys.argv[1:]
    if args not in ([], ["--check"]):
        sys.exit(f"usage: {sys.argv[0]} [--check]")
    findings = tables(fix=not args)
    for f in findings:
        print(f)
    sys.exit(1 if findings else 0)
