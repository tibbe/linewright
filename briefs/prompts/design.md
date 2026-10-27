# Your task

Design the code for the {{kind}} below. You may read GitHub but must not write to it.

{{facts}}

{{item}}

{{filed_gaps}}

## Steps

### 1. Research prior art and choose a design

Read the {{kind}}, `GLOSSARY.md` if present, the spec and the code the {{kind}} touches. Then call the Skill tool with `linewright:software-design`, and find how the problem has been solved before and choose a design from it, as it says. Start its research sub-agents together, in the foreground, and choose from the reports they return.

### 2. Design the code

Work out what the chosen design changes in the code: what the code does today and where it falls short, the parts, what each is responsible for, where it lives, and the interfaces between them.

Keep the code's structure where it holds up. Change it where the change would add a special case to it, or where the structure makes the problem likely, so that the same kind of problem would come back.

A changed structure covers the part the change touches, judged against the whole spec rather than the {{kind}} alone.

### 3. Cut the work

{{cutting}}

### 4. Output

{{output}}

## Spec gaps

The spec is the maintainer's: change no file of it. A question the spec leaves open is a spec gap only when it is a product decision: a reasonable maintainer could want either answer, the answer changes what a user sees, and the spec's wording doesn't already settle it. Anything else is yours to decide, as the spec and the code support best.

A gap that blocks the design is the question of a `blocked` result. For a gap that doesn't, choose the reading the spec and the code support best, carry on, and list it in `spec_gaps`.
