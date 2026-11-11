# External behavior

A probe asks the real system the exact question.

- **The real thing.** Run the real operation, on each platform the framing
  names, against the same type and version of each of the backing services
  the change will run with. A fake proves the code calls the fake.
- **The exact call.** Probe the exact call the option will make: the same
  endpoint form, the same library version, the same argument spelling.
- **The actual path.** Run the actual path each criterion names and verify
  the output, not just package installation or `--version`.
- **Edge cases.** Never call a criterion met after a single happy-path test.
  Test edge cases.
- **Record.** Report the command, the observed output, and at which version.
