---
name: software-design
description:
  Find how the problem has been solved before, and choose a design from it. Use
  before implementing a feature or fixing a bug.
---

# Software design

## Process

### 1. Research

Assume the problem has been solved before, and research how in parallel
sub-agents, so each search stays out of the others' context and yours.

1. Frame the problem: the behavior the change must deliver, the quality
   attributes the spec states, and the platform it must run on and its
   language. The current design is open to change, down to the libraries and
   components it builds on.
2. Spawn one sub-agent per kind of prior art the problem has:
   - **Platform**: the platform's, framework's or library's own recommended
     approach, from its documentation or design guidelines.
   - **Products**: how other products of the same kind as ours, and others
     facing the same problem, handle this behavior.

   Give each the framing and this brief: "Search the web for how this problem is
   solved. Report every principled design the sources take, each with its
   trade-offs, the evidence of its adoption, why its adopters chose it, and the
   sources it comes from. Evidence of adoption: the platform providing or
   documenting it; products like ours using it; and, for a library to adopt, its
   GitHub stars, package downloads and recent releases. Under 400 words."

### 2. Choose

Compare the reported designs and, for each decision, follow the consensus: the
design most sources converge on, counting a solution the platform documents for
this problem as strong evidence. Any other design, whether fewer sources take it
or none do, has to win against it.

Before choosing, write down the case against each design. The case against the
consensus first says why its adopters chose it, then why that reasoning doesn't
apply here. Strike out any reason, for or against a design, that:

- counts the size of the change;
- leans on what the code already does;
- is circular: it assumes the choice it argues for.

Choose on the reasons that remain.

## Output

End with, restating what you wrote down, one block per decision:

```markdown
## {the decision, as a question}

### Context and Problem Statement

{two or three sentences}

### Considered Options

* {title of option 1}
* {title of option 2}

### Decision Outcome

Chosen option: "{title of option 1}", because {justification, in one sentence}.
{Follows the consensus. | Departs from the consensus, "{title of option}",
because {why, in one sentence}.}

### Pros and Cons of the Options

#### {title of option 1}

{what it is, in a sentence or two}. Taken by {the sources that take it}.

* Good, because {why its adopters chose it}
* Bad, because {an argument}
* ~~{a reason, for or against}~~ (struck: {the rule that struck it})

#### {title of option 2}

{what it is, in a sentence or two}. Taken by {the sources that take it}.

* Good, because {why its adopters chose it}
* Bad, because {an argument}
```

One Good line per option: why its adopters chose it.
