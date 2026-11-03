# The formal model of the spec

A formal model of Linewright's spec, checked exhaustively within small bounds;
here in [Quint](https://quint-lang.org), checked with TLC. It changes in the
same commit as the spec it encodes. To change it or check it, use the
`verify-spec` skill.

The model's files are their own documentation:

- `linewright.qnt`: the spec's rules, each definition named after the spec's
  term and quoting the clause it encodes; its header gives the letters for the
  spec files, names the sections the model encodes in full and lists the
  assumptions (A1, A2, …), the readings the spec leaves open. Each property
  (S1 to S7) is stated on its definition.
- The spec's tables are not copied into the model: `scripts/fix
  model-tables` generates each table's definition from the spec, between a
  `// spec-table` directive and `// spec-table end`, and `scripts/check
  model-tables` fails where it is out of date. The model gives one
  `when_<cell>` predicate per When cell, one `does_<cell>` per Linewright
  cell and a `result_<cell>` per result that is not a literal, each named by
  the cell's text. `scripts/model_tables.py` describes the directives.
- `pipeline.qnt`, `cut.qnt`, `spec_change.qnt`: the three models, each header
  giving its items, bounds and starts; and their scenarios, named after the
  regressions (S8) and issues (S9) they replay.
- `pull_request.qnt`: Working a pull request, a model of its own with its own
  properties.
- `facts.qnt`: what GitHub records about an item.
- `package.json`, `package-lock.json`: the pinned Quint.

`scripts/check --help` lists the ways to check the model. To run Quint by hand,
use the tools `scripts/check` installed under
`${XDG_CACHE_HOME:-$HOME/.cache}/linewright` (`QUINT_HOME` at `quint-home`
there, its Java 21 first on the `PATH`), from a scratch directory: Apalache
writes its output where it runs.
