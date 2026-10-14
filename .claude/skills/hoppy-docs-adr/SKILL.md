---
name: hoppy-docs-adr
description: Use when recording a decision (ADR), updating agent-facing docs (AGENTS.md router, skills, subagents), or after adding a new area of the codebase that agents must be able to find.
---

# Decisions and AI-facing docs

## ADRs

`docs/decisions/<NNNN>-<short-title>.md`, numbered sequentially. Template:

```
# NNNN. Title
Status: Proposed | Accepted | Superseded by NNNN
Date: YYYY-MM-DD

## Context
## Decision
## Consequences
```
Keep them short. Never rewrite an accepted ADR's decision: supersede it.

## Keeping agent docs useful

- `AGENTS.md` is the router: one row per kind of task → skill, files, verify command.
  Add a row when a new area appears; remove rows for deleted areas.
- Skills (`.claude/skills/<name>/SKILL.md`): frontmatter `name` = folder name, a
  `description` that says *when* to use it. Body: where things are, rules, verify command.
  Prefer tables and paths over prose. No duplication: link to `docs/` for details.
- Subagents (`.claude/agents/*.md`): narrow role, explicit tools, explicit "done" criteria.
- Paths written in backticks are checked: `python3 tools/dev.py docs` fails on dead links.
