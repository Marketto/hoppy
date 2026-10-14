# CLAUDE.md

@AGENTS.md

## Claude Code specifics

- Skills in `.claude/skills/` load on demand; the router table in AGENTS.md says which.
- Subagents in `.claude/agents/`: delegate platform work to `android-engineer` /
  `windows-engineer` / `core-engineer`; always finish wire or file-writing changes with
  `protocol-reviewer` (read-only).
- Before saying a task is done, run the "Verify with" command from the router row and
  report its result. If a toolchain is missing (`python3 tools/dev.py doctor`), say which
  check could not run instead of claiming success.
