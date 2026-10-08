# Contracts

**Language:** English. **Status:** Proposed technical contracts for implementation agents.
Business meaning lives in `docs/`; files here freeze representation, wire shape, and
expected vectors so parsers in Rust and TypeScript behave identically.

## Files

| File | Content |
|---|---|
| `domain-vectors.json` | Synthetic money/time/text/quota vectors with independently written expected values |
| `idempotency-cases.json` | (P00-T05) Durable action-deduplication cases and sensitive-replay rules |
| `auth-cases.json` | (P00-T06) Authentication boundary cases and technical thresholds |
| `openapi.yaml` | (P00-T07) Mandatory API inventory, DTO allowlists, ownership guards |
| `authorization-cases.json` | (P00-T07) Actor/resource refusal matrix and overposting vectors |
| `scenario-tests.json` | (P00-T08) Planned test registry mapping scenarios to suites |

## Rules for every contract file

- Exact inputs and expected outputs are written independently of the implementation;
  no expected value is derived by calling the code under test.
- All examples are synthetic. No real phone number, name, address, token, or credential
  appears here or in any fixture derived from these files.
- Error `code` strings are stable. Adding a code requires a contract revision note;
  renaming a code is a breaking change.
- Money at API boundaries is always an exact decimal string; binary floating point is
  forbidden on the wire and in fixtures.
- These contracts never invent business behavior. A conflict between a contract file
  and `docs/` is resolved in favor of `docs/`, and the contract is corrected.

See [DEC-0002](../docs/engineering/decisions/DEC-0002-domain-contract.md) for the
money/time/text/refusal rationale and
[the policy values](../docs/product/principles-and-policies.md#23-policy-values-and-interpretation)
for canonical limits.
