---
on:
  schedule: "0 6 * * 1"
timeout: 15m
max-issues: 2
---
Check whether the `Dockerfile` pins the newest Claude Code and gh, and file an
issue to upgrade each one it does not.

1. Read the pins, `CLAUDE_CODE_VERSION` and `GH_VERSION`, in the `Dockerfile`.

2. Read the newest Claude Code from its `stable` apt channel, the highest
   `Version` in the index, without the Debian revision (`-1`). The channel
   already holds releases back about a week, so take its newest as it is.

   ```sh
   curl -sSf https://downloads.claude.ai/claude-code/apt/stable/dists/stable/main/binary-amd64/Packages
   ```

3. Read the newest gh release, its `tag_name` without the `v` and its
   `published_at`:

   ```sh
   curl -sSf https://api.github.com/repos/cli/cli/releases/latest
   ```

4. File nothing for a tool when any of these holds:
   - Its pin already names the newest version.
   - For gh, the release came out less than 3 days ago.
   - An issue to upgrade the tool is open, whatever version it names.

5. Otherwise file one `ready-for-agent` issue per tool, titled "Upgrade Claude
   Code to VERSION" or "Upgrade gh to VERSION", saying to set its pin in the
   `Dockerfile` to VERSION, along with any checksum pinned with it. Read the
   changes between the pinned version and VERSION, in Claude Code's
   `CHANGELOG.md` (https://github.com/anthropics/claude-code) or gh's release
   notes, and link them. List the ones Linewright could notice, and say to
   adapt to them:
   - For Claude Code: its flags, its stream-json output, its transcripts and
     how it loads settings and skills.
   - For gh: commands the agents' briefs and skills use.
