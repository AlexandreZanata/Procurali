# Decision records

The initial recommended business choices and their alternatives/rationale live in [principles and policies](../product/principles-and-policies.md) and relevant workflow/commercial topics. They remain recommendations unless an explicit product decision approves them. Do not create a second copy of those baseline choices here.

## When to create a record

Use a decision record for a consequential new choice or a change to the baseline: an invariant, privacy boundary, state transition, numeric policy, commercial term, scope boundary, or explicitly requested future technical decision.

An organizational move or spelling correction belongs in the [changelog](../maintenance/changelog.md), without an invented business decision. A later technical decision must be labeled technical and must not turn technology choices into business requirements.

## Identity and status

Use a unique increasing identifier such as `DEC-0001` and a descriptive filename such as `DEC-0001-request-renewal-policy.md`. Numbers are identities, not a requirement to create a decision now. No separate decision records have been created by this documentation reorganization.

Record status as proposed, approved, rejected, or superseded. Identify approval evidence when status is approved. A proposed record contains options and a recommendation; it does not silently replace a rule. A superseded decision keeps a link to its successor and a reason.

## Required information

Use the [decision template](../templates/decision-record.md) to document the problem, options, recommended/selected choice, rationale, release scope, effective timing, existing-resource handling, impacted paths and `INV-*`/`AC-*`/`EC-*` identifiers, and verification obligations.

Once a decision is authorized within the task scope, reconcile all affected canonical documents, navigation, manifest entries, and acceptance behavior. Add the decision record to the manifest and the significant change to the changelog. The rule's canonical topic stays the source of current behavior; the decision record explains why it changed.

## Related documents

- [Documentation index](../README.md).
- [Agent workflow](../maintenance/agent-workflow.md).
- [Principles and policies](../product/principles-and-policies.md).
