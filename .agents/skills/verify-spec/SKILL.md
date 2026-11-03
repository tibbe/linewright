---
name: verify-spec
description:
  Check the spec against its formal model in `docs/model/`. Use after changing
  a spec section the model covers, when changing the model, or when a model
  check fails.
---

# Verify the spec

1. `scripts/fix model-tables` regenerates the model's encoding of the spec's
   tables. Then `scripts/check model-tables model-quotes model-coverage`
   lists: the `when_`, `does_` and `result_` definitions a changed or added
   cell needs and the ones no cell needs any more; the model's quotes of
   clauses that changed; and the sentences of the sections it encodes in full
   that it quotes nowhere. For a clause added to any other section, grep the
   model for its section.
2. Change the model in the same commit as the spec. The three checks compare
   text: they pass once every name and quote matches, whatever the code under
   them does. For each clause that changed, read every definition that encodes
   it, the bodies of its `when_`, `does_` and `result_` definitions and any
   other code the clause governs, and make each do what the clause now says.
   A `when_` predicate reads its cell and nothing else; a clause of the spec it
   would need is encoded where the spec states it. A reading the spec leaves
   open is a new numbered assumption beside the model's others. Done when each
   changed clause's encoding reads as the clause does and all three checks
   pass.
3. Run `scripts/check model MODEL NAME...` for the scenarios, invariants, flap
   and temporal properties the change bears on.
4. A violation is a finding: fix the model where it misreads the spec;
   otherwise report the trace to the human. Weaken no property, bound or
   assumption without their agreement.
5. Break each property you add or change, see it go red, and revert.
6. `scripts/check model-quick` takes about 50 minutes and `scripts/check model`
   over six hours: run one only when the human asks, in the background, and
   otherwise hand them off as still to run.
