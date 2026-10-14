# 0004. AI-first repository layout
Status: Accepted
Date: 2026-10-14

## Context
Most development will be done by AI agents. They work best with one entry point, small
task-specific instructions, a single command runner, and checks that fail loudly.

## Decision
- `AGENTS.md` is the router (task → skill → files → verify command); `CLAUDE.md` imports it.
- Task knowledge lives in `.claude/skills/*/SKILL.md`; roles in `.claude/agents/*.md`.
- `tools/dev.py` is the only command agents need; `dev.py docs` lints the docs themselves.
- Generated assets (mascots) come from code, so agents edit one source.

## Consequences
New areas must add a router row and, if non-trivial, a skill. Docs drift is caught by
`dev.py docs` in CI.
