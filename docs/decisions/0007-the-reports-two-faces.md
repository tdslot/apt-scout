# The report has two faces: one for a person, one for a machine

The run prints one block on stdout that a person reads and a harness matches. Those two uses pull in opposite
directions, and the rules that resolve the pull are in `docs/spec/output.md`; this record keeps the reasoning.

## Why the accent carries weight as well as colour

In the 256-colour form the Ubuntu accent is `166`, which sits close to the red that a rejection reason and a
`-` diff line carry. Two colours alone left structure looking like "this is wrong" on a dark terminal, and
bold is most terminals' bright variant, which pulls the accent away from that dark-red end. The spinner keeps
the plain weight because no red sits beside it: the fix is local to the places where the two meanings would
otherwise collide.

## Why the line a person copies is never styled

The `sudo apt-scout` line a non-root run prints is never styled, because a user copies it. That is the whole
reason: styling is for reading, and this line exists to be pasted somewhere that would keep the escape
sequences.

## Why the accent has a named fallback

`COLORTERM` picks between the 24-bit and the 256-colour form, and the fallback is named rather than inferred
because a terminal will not complain: an unsupported sequence is dropped silently, so a run that guessed wrong
would look styled to its author and plain to everyone else. Naming the fallback makes the wrong guess visible
in the code instead of invisible on the screen.

## Why the plain output is the contract

A harness matches the plain output, so every styled form has to reduce to the same lines and the same order.
That is why no timeout, request, measurement, rejection or join order may change because a frame was drawn, and
why a write that fails is dropped as before rather than turned into an error for the reader's benefit: the two
faces show the same facts, and only the escapes differ.

**Spec**: `docs/spec/output.md`, `OUT-4` to `OUT-9`. **Aliases**: none.
