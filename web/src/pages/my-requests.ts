/**
 * Own requests: server-owned state with working navigation and sharing.
 * State, deadlines, expiry, and the eligible-action list render verbatim
 * from the server summary; no local timer grants expired behavior and no
 * renewal, outcome, or removal button exists here (those endpoints are a
 * documented pending surface, so no fake action is offered). Sharing a
 * demand records a real share-intent fact through its endpoint.
 */
import { ApiClient, ContractError, createMutationIntent } from "../api/client.js";
import { renderErrorSummary } from "../components/error-summary.js";

export interface OwnRequest {
  id: string;
  title: string;
  state: string;
  cycle: number;
  deadline: string | null;
  expired: boolean;
  actions: string[];
}

interface ListBody {
  requests?: Array<{
    id?: unknown;
    title?: unknown;
    state?: unknown;
    cycle_number?: unknown;
    deadline?: unknown;
    expired?: unknown;
    actions?: unknown;
  }>;
}

/** List the owner's demands newest-first with server-derived standing. */
export async function fetchMyRequests(client: ApiClient): Promise<OwnRequest[]> {
  const body = await client.get<ListBody>("/api/v1/requests?limit=50");
  const items: OwnRequest[] = [];
  for (const row of body.requests ?? []) {
    if (typeof row.id !== "string" || typeof row.title !== "string") {
      continue;
    }
    items.push({
      id: row.id,
      title: row.title,
      state: typeof row.state === "string" ? row.state : "unknown",
      cycle: typeof row.cycle_number === "number" ? row.cycle_number : 0,
      deadline: typeof row.deadline === "string" ? row.deadline : null,
      expired: row.expired === true,
      actions: Array.isArray(row.actions)
        ? row.actions.filter((action): action is string => typeof action === "string")
        : [],
    });
  }
  return items;
}

/** Record one share-intent fact for an owned demand. */
export async function shareRequest(
  client: ApiClient,
  requestId: string,
  channel: string,
): Promise<string> {
  const body = await client.mutate<{ link?: unknown }>(
    "POST",
    `/api/v1/public/requests/${requestId}/share`,
    { channel },
    createMutationIntent(),
  );
  if (typeof body.link !== "string") {
    throw new ContractError("invalid_field", 500, "share receipt carries no link");
  }
  return body.link;
}

/** Render owned demands with state, server deadline, actions, and share. */
export function renderMyRequests(
  container: HTMLElement,
  client: ApiClient,
  items: OwnRequest[],
): void {
  container.textContent = "";
  if (items.length === 0) {
    const empty = document.createElement("p");
    empty.textContent = "No requests yet.";
    container.appendChild(empty);
    return;
  }
  const summary = document.createElement("div");
  container.appendChild(summary);
  const list = document.createElement("ul");
  container.appendChild(list);
  for (const item of items) {
    const row = document.createElement("li");
    const title = document.createElement("strong");
    title.textContent = item.title;
    row.appendChild(title);
    const standing = document.createElement("p");
    const deadline = item.deadline ?? "no deadline reported";
    standing.textContent =
      `State ${item.state}, cycle ${item.cycle}, deadline ${deadline}` +
      (item.expired ? ", expired" : "");
    row.appendChild(standing);
    if (item.actions.length > 0) {
      const eligible = document.createElement("p");
      eligible.textContent = `Eligible next: ${item.actions.join(", ")}`;
      row.appendChild(eligible);
    }
    const open = document.createElement("a");
    open.href = `/r/${item.id}`;
    open.textContent = "Open";
    row.appendChild(open);
    const share = document.createElement("button");
    share.type = "button";
    share.textContent = "Share";
    share.addEventListener("click", () => {
      share.disabled = true;
      shareRequest(client, item.id, "copy").then(
        () => {
          share.disabled = false;
          renderErrorSummary(summary, []);
        },
        (error: unknown) => {
          share.disabled = false;
          renderErrorSummary(summary, [
            error instanceof ContractError ? error.message : "Could not share.",
          ]);
        },
      );
    });
    row.appendChild(share);
    list.appendChild(row);
  }
}
