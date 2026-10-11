# Linewright

See `README.md` for what it does. It is written in Rust and built with cargo.

## Specs

This project uses specification-driven development.

- `docs/specs/product.md`: the product spec: problem, user stories and product
  decisions.
- `docs/specs/architecture.md`: the architecturally significant decisions, the
  ones costly to change later, e.g. languages and frameworks.
- `GLOSSARY.md`: the vocabulary the specs and the code use, each term defined
  once.

How the code does what the specs say belongs in the code and its comments. Write
specs as what Linewright should do, not as an account of what follows from it
or of what other systems such as GitHub do. Keep the specs in step with
user-visible behavior changes.

Changes to specs require human approval.

## Coding standards

`CODING_STANDARDS.md` should always be read before any code review.

## Area guides

Before working in one of these areas, read its guide:

- [Commits](./docs/commits.md)
- [Checks](./docs/checks.md)
- [Developing Linewright](./docs/developing.md): tuning the briefs, the tests
