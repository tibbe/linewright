---
name: software-design
description:
  Find how the problem has been solved before, and choose an option from it.
  Use before implementing a feature or fixing a bug.
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
2. Spawn one sub-agent per kind of prior art the problem has, together, in
   the foreground:
   - **Platform**: the platform's, framework's or library's own recommended
     approach, from its documentation or design guidelines.
   - **Products**: how other products of the same kind as ours, and others
     facing the same problem, handle this behavior.

   Give each the framing and this brief: "Search the web for how this problem is
   solved. Report every principled option the sources take, each with its
   trade-offs, the evidence of its adoption, why its adopters chose it, and the
   sources it comes from. Evidence of adoption: the platform providing or
   documenting it; products like ours using it, with each product's users,
   downloads or GitHub stars; and, for a library to adopt, its GitHub stars,
   package downloads and recent releases. Under 400 words."

### 2. Criteria

Once every research sub-agent has returned its report, list every option the
reports give. Then, for each decision, write its criteria: the quality
attributes the framing states, and performance, wherever the reports'
trade-offs show the options can differ on them. Word each so that more of it is
better, one concern to each. Mark as a must-have any the spec requires
outright, and rank the rest High, Medium or Low by how much the change's
success depends on them.

### 3. Prototype

Spawn one sub-agent per option, together, in the foreground. Give each:

- the framing from step 1 of Research, word for word;
- the option, as the research reports give it;
- its decision's criteria, as you wrote them;
- this brief: "Call the Skill tool with `linewright:prototype`, and build
  the smallest version of the option above that does what the framing
  states and shows how it fares on the criteria above. End with what
  happened, in this form and nothing else, no code,
  under 80 words: `**{the option}**: {works | works, with … | doesn't
  work}. Ran {what} against {what}, {how many times}, at {the size and
  shape}: {the observation on each criterion}.`"

### 4. Choose

For each decision:

1. Rule out any option that fails a must-have.
2. Of the rest, take the consensus as the reference: the most widely adopted,
   by the evidence of adoption the reports give, counting a solution the
   platform documents for this problem as strong evidence.
3. Rate each other option against the reference on each criterion as better,
   same, worse or unknown, with the reason and where it comes from: its
   prototype or a research report.
4. Where an unknown could change the choice, settle it with a prototype, as in
   Prototype, before choosing.
5. Follow the reference unless another option's better ratings outweigh its
   worse ones, weighed by each criterion's rank and by how far apart the
   options are on it, not by their count.

Before choosing, write down the case against each option. The case against the
consensus first says why its adopters chose it, then why that reasoning doesn't
apply here. Strike out any reason, for or against an option, that:

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

### Decision Drivers

* {a criterion} ({must-have | High | Medium | Low})

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

{its prototype's result, as the sub-agent gave it}

* Good, because {why its adopters chose it}
* Bad, because it fails the must-have {a criterion}: {the reason},
  from {its prototype | a research report}
* {Good | Neutral | Bad}, because it is
  {better than | the same as | worse than} "{the reference}" on {a criterion}:
  {the reason}, from {its prototype | a research report}
* Unknown on {a criterion}, because
  {what neither its prototype nor a research report shows}
* Bad, because {an argument}
* ~~{a reason, for or against}~~ (struck: {the rule that struck it})

#### {title of option 2}

…
```

From the research, one Good line per option: why its adopters chose it. A Bad
line for each must-have an option fails. Every option but the reference has one
line per criterion, comparing it with the reference: Good, Neutral, Bad or
Unknown.
