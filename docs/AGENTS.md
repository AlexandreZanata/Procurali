# Documentation instructions

These instructions apply to everything under `docs/` together with the repository's root `AGENTS.md`.

1. Keep all explanatory content in English and maintain a single canonical location for each rule.
2. Start with `README.md`; use `manifest.json` for exact document paths, scope, and relationships.
3. `BUSINESS_LOGIC_SPECIFICATION.md` is a navigation index. Edit the linked topic, not the index, when changing business behavior.
4. Preserve the original section numbers and stable `INV-*`, `AC-*`, and `EC-*` identifiers. New supplemental documents do not require renumbering the original specification.
5. Canonical business documents declare `document_id`, `status`, `scope`, `source_sections`, and `last_updated` in their metadata header.
6. Keep manifest identity, path, status, scope, source sections, and related-document entries consistent with canonical metadata.
7. Keep shared launch limits in `product/principles-and-policies.md` and topic-specific policies in their owning topic, as mapped by `product/policy-index.md`. Downstream repetitions explain context; they are not competing policy sources.
8. If a rule changes, reconcile lifecycle, invariant, permission, workflow, edge-case, acceptance, and measurement implications before marking the change complete.
9. Preserve the distinction between a recommended baseline, a proposed decision, an approved decision, and verified implementation.
10. Never change checkbox completion or implementation status without supporting evidence.
11. Keep MVP boundaries explicit in mixed-scope documents; commercial planning does not make paid features mandatory for the free MVP.
12. Keep relative links and old entry-point anchors working when reorganizing files.
13. Record significant changes in `maintenance/changelog.md` and consequential decisions according to `decisions/README.md`.
14. Use the templates in `templates/` for new topic documents, decision records, and substantive task handoffs.
15. Follow `maintenance/agent-workflow.md` for validation and report only checks actually performed.
