# Benchmark

A benchmark proves only the command that was run.

- **Size and shape.** Measure with realistic data sizes and concurrency, as
  the framing gives them. Synthetic inputs whose shape differs from real ones
  (size, cardinality, nesting) can invert a conclusion.
- **Rounds.** Run each option at least 5 times, interleaved so that all see
  the same noise: every option once per round, randomly reordered for each
  round.
- **Numbers.** Report the median and range, and never a ratio without the
  absolute values.
- **Platform.** Run on each platform the framing names. Rankings from another
  platform transfer when the hot path is platform-neutral code: parsing,
  hashing, allocation patterns, algorithmic work.
