# Your task

Design the code for the {{kind}} below. You may read GitHub but must not write to it.

{{facts}}

{{item}}

{{filed_gaps}}

## Steps

### 1. Research prior art and choose an option

Read the {{kind}}, `GLOSSARY.md` if present, the spec and the code the {{kind}} touches. Then call the Skill tool with `linewright:software-design`, and find how the problem has been solved before and choose an option from it, as it says. Start its research sub-agents together, in the foreground, and choose from the reports they return.

### 2. Design the code

A design names the modules the change touches. For each, it says what the module is responsible for and its interface to the other modules. A module is anything with an interface and an implementation: a method or function, a class, a package, a service or a library the code uses. Its interface is everything the other modules must know of it. Everything else the module hides, and the implementer decides it.

Some decisions affect more than one module, and must be made before work on the modules can begin. Those are the design's. The first is the seam, where the option attaches to the code. Choose each as the software-design skill chooses between options: list the ways it could go, write the case against each, and take the one that wins.

Keep the code's structure where it holds up. Change it where the change would add a special case to it, or where the structure makes the problem likely, so that the same kind of problem would come back.

A changed structure covers the modules the change touches, judged against the whole spec rather than the {{kind}} alone.

### 3. Cut the work

{{cutting}}

### 4. Output

{{output}}

## Spec gaps

The spec is the maintainer's: change no file of it. A question the spec leaves open is a spec gap only when it is a product decision: a reasonable maintainer could want either answer, the answer changes what a user sees, and the spec's wording doesn't already settle it. Anything else is yours to decide, as the spec and the code support best.

A gap that blocks the design is the question of a `blocked` result. For a gap that doesn't, choose the reading the spec and the code support best, carry on, and list it in `spec_gaps`.
