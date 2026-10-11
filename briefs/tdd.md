<!-- Forked from mattpocock/skills, skills/engineering/tdd with tests.md and
mocking.md (MIT, see LICENSE). Made non-interactive for Linewright. -->

# Test-Driven Development

TDD is the red → green loop. This is the reference that makes that loop
produce tests worth keeping: what a good test is, where tests go, the
anti-patterns, and the rules of the loop. Every section applies on every
cycle: consult them before and during the loop, not after.

Read `GLOSSARY.md` (if it exists) so test names and interface vocabulary match
the project's domain language, and respect the architecture decisions in the
area you're touching.

## What a good test is

Tests verify behavior through public interfaces, not implementation details.
Code can change entirely; tests shouldn't. A good test reads like a
specification: "user can checkout with valid cart" tells you exactly what
capability exists, and it survives refactors because it doesn't care about
internal structure.

- Tests behavior users and callers care about, through the public API only.
- Survives internal refactors; describes WHAT, not HOW.
- One logical assertion per test.

```typescript
// GOOD: observable behavior
test("user can checkout with valid cart", async () => {
  const cart = createCart();
  cart.add(product);
  const result = await checkout(cart, paymentMethod);
  expect(result.status).toBe("confirmed");
});

// BAD: implementation detail
test("checkout calls paymentService.process", async () => {
  const mockPayment = jest.mock(paymentService);
  await checkout(cart, payment);
  expect(mockPayment.process).toHaveBeenCalledWith(cart.total);
});
```

## Seams: where tests go

A **seam** is the public boundary you test at: the interface where you observe
behavior without reaching inside. Tests live at seams, never against
internals.

**Test only at agreed seams.** Nobody is there to agree them with you, so take
them from the repository: its coding standards (a "where tests go" section,
for instance), the seams its existing tests already use, and the public
interfaces the ticket names. Write down the seams you will test before writing
the first test, and keep to them. Agreeing the seams up front is how testing
effort lands on the critical paths and complex logic instead of every edge
case.

## When to mock

Mock at **system boundaries** only: external APIs, time and randomness,
sometimes databases (prefer a test database) and the file system. Don't mock
your own modules or internal collaborators. Design boundaries to be easy to
fake: pass dependencies in rather than creating them inside, and prefer
specific functions per external operation (`getUser`, `createOrder`) over one
generic fetcher whose fake needs conditional logic.

## Anti-patterns

- **Implementation-coupled**: mocks internal collaborators, tests private
  methods, or verifies through a side channel (querying the database instead
  of using the interface). The tell: the test breaks when you refactor but
  behavior hasn't changed.
- **Tautological**: the assertion recomputes the expected value the way the
  code does (`expect(add(a, b)).toBe(a + b)`, a snapshot derived by hand the
  same way, a constant asserted equal to itself), so it passes by construction
  and can never disagree with the code. Expected values must come from an
  independent source of truth: a known-good literal, a worked example, the
  spec.
- **Horizontal slicing**: writing all tests first, then all implementation.
  Bulk tests verify _imagined_ behavior. Work in **vertical slices** instead:
  one test → one implementation → repeat, each test a **tracer bullet** that
  responds to what the last cycle taught you.

## Rules of the loop

- **Red before green.** Write the failing test first, watch it fail, then
  write only enough code to pass it. Don't anticipate future tests or add
  speculative features.
- **One slice at a time.** One seam, one test, one minimal implementation per
  cycle.
- **Refactoring is not part of the loop.** It belongs to the review stage, not
  the red → green cycle.
