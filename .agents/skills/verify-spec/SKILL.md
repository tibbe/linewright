---
name: verify-spec
description:
  Check the spec against its formal model in `docs/model/`. Use after changing
  a spec section the model covers, when changing the model, or when a model
  check fails.
---

# Verify the spec

1. `scripts/check model-quotes` lists the model's quotes of clauses that
   changed. For added clauses, grep the model for their section.
2. Change the model in the same commit as the spec, until `model-quotes`
   passes. A reading the spec leaves open is a new numbered assumption
   beside the model's others.
3. Run `scripts/check model-quick`, and `scripts/check model MODEL NAME...` for
   the flap and temporal properties the change bears on.
4. A violation is a finding: fix the model where it misreads the spec;
   otherwise report the trace to the human. Weaken no property, bound or
   assumption without their agreement.
5. Break each property you add or change, see it go red, and revert.
6. `scripts/check model` takes about two hours: run it in the background, or
   hand it off as still to run.
