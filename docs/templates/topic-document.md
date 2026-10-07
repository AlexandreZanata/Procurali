# Template: Canonical topic document

This is a reusable template, not an active business rule. Copy its structure into a focused topic file and replace instructional text with complete content.

## Metadata

Include a metadata header with `document_id`, `status`, `scope`, `source_sections`, and `last_updated`. Choose a unique document identity, a truthful status, and `mvp`, `post-mvp`, `future`, or `mixed` scope. Use an empty source-section list for a genuinely new topic rather than inventing an original section number.

## Purpose and authority

Explain the business question owned by this document and which related documents own its surrounding constraints. State whether the topic is a recommendation, an approved baseline, or future planning.

## Vocabulary and actors

Reference the glossary and identify relevant actors, resource owners, and business inputs. Do not copy the whole glossary or role catalog.

## Business behavior

Specify preconditions, validations, outcomes, allowed transitions, refusals, deadlines, visibility, notices, and audit/event implications. Link to canonical policy values and invariants instead of defining an inconsistent duplicate.

## Decisions and release scope

Explain options, recommendation, rationale, and what belongs to MVP versus later releases. Link to a decision record if a consequential new choice is being introduced.

## Edge cases and acceptance

Identify affected `EC-*` and `AC-*` entries. Add new stable identities only when a distinct scenario is necessary. Describe observable business results rather than implementation assumptions.

## Related documents

Link to the documentation index, relevant canonical topics, and agent maintenance workflow. Update the manifest, index, and changelog as appropriate before considering the new topic complete.
