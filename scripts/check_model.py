"""`scripts/check model-quotes|model-coverage` checks the model's comments
against the spec, printing `path:line: <why>` for each finding and exiting
with 1 if it printed any.

`quotes`: every quote in a model comment is in the spec, its parts around an
ellipsis each in it.

`coverage`: every sentence of each section the model's header says it encodes
in full ("Encoded in full: R ## State, ...") is quoted in the model, and every
table in one has a spec-table directive."""

import os, re, sys

# -I leaves the script's directory off the path:
# https://docs.python.org/3/using/cmdline.html#cmdoption-I
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from spec import (
    Finding,
    LETTER,
    MODEL_FILES,
    QUOTE,
    SPEC_FILES,
    comment_runs,
    directives,
    letters_of,
    line_of,
    plain,
    quote_parts,
    sections,
    sentences_in,
    tables_in,
)

CITED_SECTION = re.compile(r"\b([A-Z]{1,2}) (#{2,6} [^,.;]+)")
ENCODED_IN_FULL = re.compile(r"Encoded in full: (.*?)\.(?:\s|$)")
MIN_QUOTE_WORDS = 3


def check_quotes():
    spec = plain(" ".join(open(f).read() for f in SPEC_FILES))
    findings = []
    for path in MODEL_FILES:
        for run in comment_runs(path):
            for m in QUOTE.finditer(run.text):
                if all(p in spec for p in quote_parts(m.group(1))):
                    continue
                quote = re.sub(r"\s+", " ", m.group(1))
                findings.append(f'{path}:{line_of(run, m.start())}: "{quote}" is not in the spec')
    return findings


def check_coverage():
    findings = []
    runs = [r for p in MODEL_FILES for r in comment_runs(p)]
    letters = {a: "docs/specs/" + f for r in runs for a, f in LETTER.findall(r.text)}
    spec = {a: sections(f) for a, f in letters.items()}
    claimed = []
    for run in runs:
        for m in ENCODED_IN_FULL.finditer(run.text):
            for c in CITED_SECTION.finditer(m.group(1)):
                letter, heading, line = c.group(1), c.group(2).strip(), line_of(run, m.start(1) + c.start())
                if letter not in spec:
                    findings.append(f"{run.path}:{line}: {letter} names no spec file")
                elif heading not in spec[letter]:
                    findings.append(f"{run.path}:{line}: {letters[letter]} has no heading {heading}")
                else:
                    claimed.append((f"{letter} {heading}", spec[letter][heading]))
    quotes = [p for r in runs for q in QUOTE.findall(r.text) for p in quote_parts(q)]
    directed = set()
    for path in MODEL_FILES:
        file_letters = letters_of(path)
        try:
            for d in directives(path):
                if d.letter in file_letters:
                    directed.add((file_letters[d.letter], d.heading, d.ordinal))
        except Finding:
            pass
    for where, section in claimed:
        for line, sentence in sentences_in(section):
            if not any(q == sentence or (len(q.split()) >= MIN_QUOTE_WORDS and (q in sentence or sentence in q)) for q in quotes):
                findings.append(f'{section.path}:{line}: "{sentence}", in {where}, which the model encodes in full, is not quoted in it')
        for k, table in enumerate(tables_in(section), 1):
            if (section.path, section.heading, k) not in directed:
                findings.append(f"{section.path}:{table.line}: the table in {where}, which the model encodes in full, has no spec-table directive")
    return findings


if __name__ == "__main__":
    args = sys.argv[1:]
    checks = {"quotes": check_quotes, "coverage": check_coverage}
    if len(args) != 1 or args[0] not in checks:
        sys.exit(f"usage: {sys.argv[0]} quotes | coverage")
    findings = checks[args[0]]()
    for f in findings:
        print(f)
    sys.exit(1 if findings else 0)
