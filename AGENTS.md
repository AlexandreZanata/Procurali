# Agent instructions

## Scope

These instructions apply throughout this repository. Follow additional `AGENTS.md` files in the directory you edit. Current user instructions take precedence over repository guidance.

## Start every task

1. Inspect the current files and working changes; preserve unrelated work.
2. Read `docs/README.md` and `docs/maintenance/project-status.md`.
3. Use the task routes or `docs/manifest.json` to locate the relevant canonical documents.
4. Read the relevant invariants, lifecycle rules, permissions, edge cases, and acceptance scenarios before changing behavior.

## Product baseline

- The principal chain is Request → Offers → Contact → Outcome.
- Keep basic buyer/seller participation free and buyer-controlled WhatsApp contact private until the authorized handoff.
- Preserve the difference between contact initiation, observed feedback, declared resolution, and a verified transaction.
- Preserve explicit MVP/post-MVP/future boundaries.
- The current phase is business specification and documentation organization. A documentation task does not authorize selecting technologies, writing application code, or designing screens.
- When implementation is explicitly requested later, record technical decisions separately and keep business behavior traceable to its specification.

## Maintain the repository

- Write documentation, repository instructions, identifiers, and explanatory examples in English. Proper product/place names may remain unchanged.
- Edit the canonical topic once; use links elsewhere instead of copying rules or policy values.
- Preserve `DOC-*`, `INV-*`, `AC-*`, and `EC-*` identities; use new identities for additions.
- Keep relative Markdown links, related-document navigation, and `docs/manifest.json` consistent with file changes.
- Follow `docs/maintenance/agent-workflow.md` for impact analysis, verification, change records, and handoffs.
- Record material business decisions using `docs/decisions/README.md`; never present an assumption as an approved decision.
- Do not claim a feature is implemented because its documentation exists. Update project status only from evidence.
- When implementation exists, verify behavior with the relevant acceptance scenarios and appropriate project checks.

## Finish a task

Report changed files, verified behavior, checks actually run, and any unresolved decision. Leave enough context for another agent to continue without relying on this chat. Do not commit, publish, or send external messages unless the task authorizes it.
