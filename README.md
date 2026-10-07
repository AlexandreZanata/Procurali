# Procuro Aí

Procuro Aí is a local reverse marketplace: buyers publish what they need and their maximum budget, nearby sellers submit offers, and buyers choose whom to contact through WhatsApp.

**Product promise:** “Say what you are looking for and how much you want to pay. People who have it for sale can find you.”

## Project documentation

Start at the [documentation index](docs/README.md). Documentation is split into focused, canonical files so AI agents can find and update only the context relevant to their task.

- [Agent instructions](AGENTS.md) define repository-wide working conventions.
- [Agent maintenance workflow](docs/maintenance/agent-workflow.md) explains discovery, editing, verification, and handoffs.
- [Document manifest](docs/manifest.json) provides machine-readable document identities, paths, scope, and relationships.
- [Business Logic Specification index](docs/BUSINESS_LOGIC_SPECIFICATION.md) preserves the original 27-section reading order.
- [Project status](docs/maintenance/project-status.md) distinguishes specified behavior from implemented behavior.

The business baseline deliberately makes no technology, programming language, framework, database, or infrastructure decisions and contains no implementation code or visual screen designs.

## Initial scope

The mandatory MVP validates one complete loop:

**Purchase request → seller offers → buyer-initiated WhatsApp contact → buyer-reported outcome.**

Individuals use the basic marketplace for free. Professional monetization is planned through Radar Pro, which helps sellers discover relevant demand automatically without charging for basic contact access.

## How to use the specification

1. Read the repository instructions and documentation index.
2. Follow the task routes to the relevant canonical topic and related domain rules.
3. Preserve existing invariant, acceptance, and edge-case identifiers when editing.
4. Update related topics, navigation, and change history when a change crosses documents.
5. Keep mandatory MVP, post-MVP, and future scope explicit and separate.

All numeric thresholds in the specification are recommended starting policies, with a rationale and a review condition. They are not claims of validated product-market fit.
