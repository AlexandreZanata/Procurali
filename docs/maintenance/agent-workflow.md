# Agent maintenance workflow

**Scope:** Documentation and any later explicitly requested implementation work.  
**Current project phase:** Business specification; see [project status](project-status.md).

This workflow is intended for AI agents working across sessions. It provides persistent project context without requiring a previous chat transcript or a particular agent product. Follow the current task and repository instructions; routine authorized edits do not require an extra approval ceremony.

## 1. Discover the relevant context

1. Inspect current working changes and preserve unrelated user or agent work.
2. Read [root instructions](../../AGENTS.md), [documentation instructions](../AGENTS.md), [index](../README.md), and [project status](project-status.md).
3. Find the task route, canonical path, document identity, and release scope using the index or [manifest](../manifest.json).
4. Read the primary topic and its relevant related documents. Inspect affected invariants, transitions, permissions, edge cases, and acceptance scenarios.
5. State the intended result and distinguish a wording/organization change from a behavior change.

Read only the necessary initial context. An offer change generally needs offers, request validity, state transitions, invariants, permissions, and matching acceptance/edge-case entries; it need not load future B2B planning unless that change affects it.

## 2. Identify the canonical owner

- Product purpose and scope: `product/overview.md`, `product/mvp-scope.md`, and `product/out-of-scope.md`.
- Definitions: `product/glossary.md`.
- Shared launch limits and decision rationale: `product/principles-and-policies.md`; use `product/policy-index.md` to find topic-specific windows, thresholds, and commercial policies.
- Conceptual entity responsibility and relations: `domain/entities.md`.
- Cross-entity operations: `domain/capabilities.md`.
- State transitions and precedence: `domain/state-transitions.md`.
- Invariant definitions: `domain/invariants.md`.
- Actor/resource authorization: `domain/permissions.md`.
- A flow's sequential behavior: its file under `workflows/`.
- Safety, moderation, and reputation: the relevant file under `trust/`.
- Commercial behavior: the relevant file under `commercial/`.
- Event meaning, metric formula, and liquidity classification: the relevant file under `measurement/`.
- Observable scenarios and boundary behavior: the relevant file under `quality/`.

Ownership is by business responsibility, not a named model or individual chat. Related documents may explain a rule in context, but a change must reconcile them with the canonical owner. The compatibility specification index and manifest never override the actual topic text.

## 3. Analyze impact before editing

For a behavior change, identify:

- Affected entities, inputs, actors, ownership, and required consent.
- Allowed and forbidden transitions, current time/deadlines, and restriction precedence.
- Existing `INV-*`, `EC-*`, and `AC-*` identifiers that must be preserved or extended.
- Privacy visibility, abusive usage, moderation history, and deletion effects.
- Events, unique-count definitions, metric denominators, cohort windows, and reputation claims.
- MVP, post-MVP, future, and commercial entitlement consequences.
- Policy values, rationale, effective timing, and behavior for existing resources.

Do not resolve a contradiction by silently choosing the easiest implementation. Establish whether current user instructions settle it, inspect the relevant sources, and document a clear recommendation if a consequential business decision remains open. Continue independent authorized work while seeking genuinely required information.

## 4. Edit with stable identities

Use small, focused Markdown documents with descriptive kebab-case filenames. Canonical business files carry metadata consistent with their manifest record. Preserve the original section numbers and existing document/invariant/scenario identities. New topics receive new identities without reusing retired ones.

Edit the canonical source and reconcile affected references in the same task. Do not leave an old business rule in an archive, a copied “summary,” or a second specification that appears equally authoritative. Link to rules and policy defaults when a repeat definition adds no value.

Use [topic](../templates/topic-document.md) and [decision](../templates/decision-record.md) templates where appropriate. Write consequential new decisions according to [decision-record guidance](../decisions/README.md). A proposed record does not change approved behavior by itself.

## 5. Keep navigation and history synchronized

When adding or moving a document:

1. Update the [index](../README.md), the relevant related-document links, and the manifest record.
2. Update any affected original-reading-order link in [the compatibility index](../BUSINESS_LOGIC_SPECIFICATION.md).
3. Preserve old entry points when they are already referenced, using a short navigation page rather than duplicate rules.
4. Add or update source-brief coverage when a requirement's canonical location changes.
5. Update `last_updated` for changed canonical topics and record a significant change in [the changelog](changelog.md).

For new canonical documents, manifest records include a unique `id`, `path`, `title`, `kind`, `status`, `scope`, `source_sections`, `summary`, and `related`. Related paths use the manifest's documented base. Existing recorded policy values remain in the canonical topic; do not store another set of thresholds in the manifest.

## 6. Verify the change

Documentation-only verification should cover:

- All local Markdown links resolve, including referenced heading anchors.
- Manifest records have unique identities/paths and point to existing files.
- Canonical metadata matches manifest identity, status, scope, and source sections.
- Existing invariant, edge-case, and acceptance identities are preserved and appear once as their canonical definitions.
- The changed rule agrees across lifecycle, invariants, permissions, workflows, boundary cases, and measurement.
- English explanatory content and explicit release scope are retained.
- No illustrative placeholder is left in an actual decision or business rule; placeholders are allowed only in explicitly labeled templates.
- Readiness checkboxes and implemented status are changed only when supporting evidence exists.

If the task later includes implementation, use the relevant acceptance scenarios and the actual project's verification tools. Do not invent a passing test result, an installed technology, or a feature completion claim. A link check is useful documentation evidence, not a functional software test.

## 7. Leave a durable handoff

For a multi-session task, save a concise handoff using [the handoff template](../templates/task-handoff.md). Put substantive handoff records under `docs/maintenance/handoffs/` when needed; create that directory when the first real record exists.

Record the objective, affected paths and stable IDs, changes made, unresolved decisions, checks actually run, and next concrete action. Do not include secrets, unnecessary personal data, or reliance on “as discussed in chat.” Routine completed edits can use the changelog and final task report instead of generating an empty handoff.

Update [project status](project-status.md) only when its claims materially change. Documentation reorganization is not application implementation. The final task report should name useful entry files, explain the concrete outcome, and distinguish verified checks from remaining work.

## Related documents

- [Documentation index](../README.md).
- [Project status](project-status.md).
- [Changelog](changelog.md).
- [Decision records](../decisions/README.md).
