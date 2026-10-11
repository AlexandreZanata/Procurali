/**
 * Safety actions: contextual block/unblock, reporting with standing,
 * and the feedback window. Blocks and reports travel their real
 * endpoints; withdrawing a report is record-only and review continues.
 * Feedback submission has no endpoint in this delivery, so the module
 * builds validated payloads and names the open window without sending.
 * No ban, score, or enforcement claim appears anywhere here.
 */
import { ApiClient, ContractError, createMutationIntent } from "../api/client.js";
import { renderErrorSummary } from "../components/error-summary.js";

export const REPORT_REASONS = [
  "fraud",
  "spam",
  "false_information",
  "prohibited_item",
  "inappropriate_behavior",
  "inappropriate_content",
  "other",
] as const;

/** Block one counterpart; new handoffs stop, history stays. */
export async function blockUser(client: ApiClient, blockedUserId: string): Promise<void> {
  await client.mutate(
    "POST",
    "/api/v1/blocks",
    { blocked_user_id: blockedUserId },
    createMutationIntent(),
  );
}

/** Lift the caller's block of one account, idempotent by user id. */
export async function unblockUser(client: ApiClient, blockedUserId: string): Promise<boolean> {
  const body = await client.mutate<{ removed?: unknown }>(
    "DELETE",
    `/api/v1/blocks/${blockedUserId}`,
    {},
    createMutationIntent(),
  );
  return body.removed === true;
}

export interface ReportInput {
  targetKind: string;
  targetId: string;
  reason: string;
  detail: string;
}

/** File one allegation; grouping and assessment stay server-side. */
export async function submitReport(client: ApiClient, input: ReportInput): Promise<string> {
  if (!(REPORT_REASONS as readonly string[]).includes(input.reason)) {
    throw new ContractError("invalid_field", 400, "unknown report reason");
  }
  if (input.reason === "other" && input.detail.trim() === "") {
    throw new ContractError("invalid_field", 400, "other reports need an explanation");
  }
  const body = await client.mutate<{ id?: unknown }>(
    "POST",
    "/api/v1/reports",
    {
      target_kind: input.targetKind,
      target_id: input.targetId,
      reason: input.reason,
      detail: input.detail,
    },
    createMutationIntent(),
  );
  if (typeof body.id !== "string") {
    throw new ContractError("invalid_field", 500, "report receipt carries no id");
  }
  return body.id;
}

export type FeedbackWindow = "not_yet" | "open" | "closed";
export type FeedbackAnswer = "yes" | "no" | "unable";

/**
 * Feedback window from the first-contact instant: opens after 24 hours
 * and closes 14 days later. Pure clock arithmetic for display only;
 * assessment itself stays server-side.
 */
export function feedbackWindowState(firstContactAt: string, now: string): FeedbackWindow {
  const start = Date.parse(firstContactAt);
  const at = Date.parse(now);
  if (Number.isNaN(start) || Number.isNaN(at)) {
    throw new ContractError("invalid_field", 400, "unparseable instant");
  }
  const openAt = start + 24 * 3_600_000;
  const closeAt = start + 14 * 24 * 3_600_000;
  if (at < openAt) return "not_yet";
  if (at > closeAt) return "closed";
  return "open";
}

/** Build one validated feedback payload (never sent here, no endpoint). */
export function buildFeedback(answer: string): FeedbackAnswer {
  if (answer !== "yes" && answer !== "no" && answer !== "unable") {
    throw new ContractError("invalid_field", 400, "unknown feedback answer");
  }
  return answer;
}

/** Render block toggle plus report form for one target. */
export function renderSafetyPanel(
  container: HTMLElement,
  client: ApiClient,
  target: { kind: string; id: string },
  counterpart: { userId: string; blocked: boolean },
): void {
  container.textContent = "";
  const summary = document.createElement("div");
  container.appendChild(summary);
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.textContent = counterpart.blocked ? "Unblock account" : "Block account";
  container.appendChild(toggle);
  toggle.addEventListener("click", () => {
    toggle.disabled = true;
    const call = counterpart.blocked
      ? unblockUser(client, counterpart.userId)
      : blockUser(client, counterpart.userId).then(() => true);
    call.then(
      () => {
        toggle.disabled = false;
        counterpart.blocked = !counterpart.blocked;
        toggle.textContent = counterpart.blocked ? "Unblock account" : "Block account";
        renderErrorSummary(summary, []);
      },
      (error: unknown) => {
        toggle.disabled = false;
        renderErrorSummary(summary, [
          error instanceof ContractError ? error.message : "Could not update the block.",
        ]);
      },
    );
  });
  const form = document.createElement("form");
  form.noValidate = true;
  container.appendChild(form);
  const reason = document.createElement("select");
  for (const option of REPORT_REASONS) {
    const item = document.createElement("option");
    item.value = option;
    item.textContent = option;
    reason.appendChild(item);
  }
  const detail = document.createElement("input");
  detail.id = "report-detail";
  const detailLabel = document.createElement("label");
  detailLabel.htmlFor = "report-detail";
  detailLabel.textContent = "Detail";
  const send = document.createElement("button");
  send.type = "submit";
  send.textContent = "Send report";
  form.append(detailLabel, reason, detail, send);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    send.disabled = true;
    submitReport(client, {
      targetKind: target.kind,
      targetId: target.id,
      reason: reason.value,
      detail: detail.value,
    }).then(
      () => {
        send.disabled = false;
        renderErrorSummary(summary, ["Report received. Review continues independently."]);
      },
      (error: unknown) => {
        send.disabled = false;
        renderErrorSummary(summary, [
          error instanceof ContractError ? error.message : "Could not file the report.",
        ]);
      },
    );
  });
}
