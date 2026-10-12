/**
 * Staff aggregates and scope honesty: operational indicators for
 * granted roles with an audited purpose, plus an explicit statement of
 * what this UI cannot do. Ban, reversal, suspension, restoration, and
 * policy-approval endpoints do not exist in this delivery, so no such
 * controls are rendered; the queue, review, and decision flows in
 * staff-cases are the only staff mutations offered anywhere.
 */
import { ApiClient, ContractError } from "../api/client.js";

export interface StaffAggregates {
  rawReports: number;
  validIncidents: number;
  eligibleInteractions: number;
  policyVersion: string;
}

/** Load operational aggregates under an audited purpose. */
export async function fetchStaffMetrics(
  client: ApiClient,
  purpose: string,
): Promise<StaffAggregates> {
  if (purpose.trim() === "") {
    throw new ContractError("invalid_field", 400, "a purpose is required");
  }
  const body = await client.get<{
    incidents?: { raw_reports?: unknown; valid_incidents?: unknown; eligible_interactions?: unknown };
    policy_version?: unknown;
  }>(`/api/v1/metrics/operational?purpose=${encodeURIComponent(purpose)}`);
  const incidents = body.incidents;
  if (
    incidents === undefined ||
    typeof incidents.raw_reports !== "number" ||
    typeof incidents.valid_incidents !== "number" ||
    typeof incidents.eligible_interactions !== "number" ||
    typeof body.policy_version !== "string"
  ) {
    throw new ContractError("invalid_field", 500, "staff aggregates malformed");
  }
  return {
    rawReports: incidents.raw_reports,
    validIncidents: incidents.valid_incidents,
    eligibleInteractions: incidents.eligible_interactions,
    policyVersion: body.policy_version,
  };
}

/** Scope honesty shown wherever staff controls render. */
export const STAFF_SCOPE_NOTE =
  "Granted safety work only: queue review and dispositions. Bans, reversals, suspensions, " +
  "restorations, and policy approvals ride future endpoints and have no buttons here.";

/** Render aggregates with the scope note and no destructive controls. */
export function renderStaffMetrics(
  container: HTMLElement,
  aggregates: StaffAggregates,
): void {
  container.textContent = "";
  const list = document.createElement("dl");
  const entries: Array<[string, string]> = [
    ["Raw reports", String(aggregates.rawReports)],
    ["Reviewed valid incidents", String(aggregates.validIncidents)],
    ["Eligible interactions", String(aggregates.eligibleInteractions)],
    ["Effective policy", aggregates.policyVersion],
  ];
  for (const [term, value] of entries) {
    const name = document.createElement("dt");
    name.textContent = term;
    const data = document.createElement("dd");
    data.textContent = value;
    list.append(name, data);
  }
  container.appendChild(list);
  const note = document.createElement("p");
  note.textContent = STAFF_SCOPE_NOTE;
  container.appendChild(note);
}
