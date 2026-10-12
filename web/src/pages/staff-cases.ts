/**
 * Staff case queue: review triage and dispositions with reasons.
 * Every mutation names its severity or disposition, reason, evidence,
 * policy version, and purpose; the server audits each step. Reporter
 * identities never render here (queue rows carry counts and business
 * context only). Ban, reversal, suspension, and restoration endpoints
 * do not exist in this delivery, so no such buttons are offered.
 */
import { ApiClient, ContractError, createMutationIntent } from "../api/client.js";
import { renderErrorSummary } from "../components/error-summary.js";

export interface CaseSummary {
  case_id: string;
  status: string;
  severity: string | null;
  report_count: number;
  reasons: string[];
}

export const SEVERITIES = ["critical", "high", "normal", "low"] as const;
export const DISPOSITIONS = ["valid", "invalid", "duplicate"] as const;

/** Page the unresolved staff queue in severity-then-age order. */
export async function fetchQueue(
  client: ApiClient,
  limit = 20,
  offset = 0,
): Promise<{ cases: CaseSummary[]; total: number }> {
  const body = await client.get<{ cases?: unknown; total?: unknown }>(
    `/api/v1/moderation/cases?limit=${limit}&offset=${offset}`,
  );
  const cases: CaseSummary[] = [];
  if (Array.isArray(body.cases)) {
    for (const row of body.cases as Array<Record<string, unknown>>) {
      if (typeof row["case_id"] !== "string" || typeof row["status"] !== "string") {
        continue;
      }
      cases.push({
        case_id: row["case_id"] as string,
        status: row["status"] as string,
        severity: typeof row["severity"] === "string" ? (row["severity"] as string) : null,
        report_count: typeof row["report_count"] === "number" ? (row["report_count"] as number) : 0,
        reasons: Array.isArray(row["reasons"])
          ? (row["reasons"] as unknown[]).filter((reason): reason is string => typeof reason === "string")
          : [],
      });
    }
  }
  return { cases, total: typeof body.total === "number" ? body.total : cases.length };
}

/** Open review on one case with triage severity and audit references. */
export async function openReview(
  client: ApiClient,
  caseId: string,
  input: { severity: string; policyVersion: string; purpose: string },
): Promise<void> {
  if (!(SEVERITIES as readonly string[]).includes(input.severity)) {
    throw new ContractError("invalid_field", 400, "unknown severity");
  }
  await client.mutate(
    "POST",
    `/api/v1/moderation/cases/${caseId}/review`,
    {
      severity: input.severity,
      policy_version: input.policyVersion,
      purpose: input.purpose,
    },
    createMutationIntent(),
  );
}

/** Decide one case under review with rationale and audit references. */
export async function decideCase(
  client: ApiClient,
  caseId: string,
  input: { disposition: string; reason: string; policyVersion: string; purpose: string },
): Promise<void> {
  if (!(DISPOSITIONS as readonly string[]).includes(input.disposition)) {
    throw new ContractError("invalid_field", 400, "unknown disposition");
  }
  if (input.reason.trim() === "") {
    throw new ContractError("invalid_field", 400, "a reason is required");
  }
  await client.mutate(
    "POST",
    `/api/v1/moderation/cases/${caseId}/decision`,
    {
      disposition: input.disposition,
      reason: input.reason,
      policy_version: input.policyVersion,
      purpose: input.purpose,
    },
    createMutationIntent(),
  );
}

/** Render the queue with counts and reasons, never reporter material. */
export function renderCaseQueue(
  container: HTMLElement,
  page: { cases: CaseSummary[]; total: number },
): void {
  container.textContent = "";
  const heading = document.createElement("h1");
  heading.textContent = `Open cases (${page.total})`;
  container.appendChild(heading);
  if (page.cases.length === 0) {
    const empty = document.createElement("p");
    empty.textContent = "No open cases.";
    container.appendChild(empty);
    return;
  }
  const list = document.createElement("ul");
  for (const item of page.cases) {
    const row = document.createElement("li");
    const title = document.createElement("strong");
    title.textContent = `${item.case_id.slice(0, 8)} — ${item.status}`;
    row.appendChild(title);
    const facts = document.createElement("p");
    facts.textContent =
      `severity ${item.severity ?? "untriaged"}, ` +
      `${item.report_count} report${item.report_count === 1 ? "" : "s"}: ${item.reasons.join(", ")}`;
    row.appendChild(facts);
    list.appendChild(row);
  }
  container.appendChild(list);
}
