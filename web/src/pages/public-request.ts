/**
 * Public request screen: anonymous understanding with safe actions.
 * Full demand renders item, budget, condition, approximate locality,
 * and the "I have this" action only when the server leaves the offer
 * action open. Anything unavailable (terminal standing, hidden,
 * suspended, prohibited, blocked, or missing) renders one generic
 * message with no protected content and no offer or contact actions.
 */
import { ApiClient, ContractError } from "../api/client.js";

export interface PublicDetail {
  id: string;
  title: string;
  budget: string;
  condition: string;
  city: string;
  regionLabel: string;
  authorName: string;
  offerAction: boolean;
}

export type PublicView =
  | { kind: "full"; detail: PublicDetail }
  | { kind: "limited"; status: string }
  | { kind: "unavailable" };

interface FullBody {
  id?: unknown;
  title?: unknown;
  budget_cents?: unknown;
  condition?: unknown;
  city_code?: unknown;
  region_code?: unknown;
  author_name?: unknown;
  offer_action?: unknown;
}

function formatBudget(cents: number): string {
  return `${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, "0")}`;
}

/**
 * Load one demand for anonymous understanding: no session required and
 * none created. Full details, terminal standing, or one generic
 * unavailable outcome — never a private field.
 */
export async function loadPublicRequest(
  client: ApiClient,
  requestId: string,
): Promise<PublicView> {
  let body: FullBody;
  try {
    body = await client.get<FullBody>(`/api/v1/public/requests/${requestId}`);
  } catch (error) {
    if (error instanceof ContractError && error.code === "not_found") {
      return { kind: "unavailable" };
    }
    throw error;
  }
  if (
    typeof body.id === "string" &&
    typeof body.title === "string" &&
    typeof body.budget_cents === "number" &&
    typeof body.condition === "string" &&
    typeof body.city_code === "string" &&
    typeof body.region_code === "string" &&
    typeof body.author_name === "string"
  ) {
    return {
      kind: "full",
      detail: {
        id: body.id,
        title: body.title,
        budget: formatBudget(body.budget_cents),
        condition: body.condition,
        city: body.city_code,
        regionLabel: body.region_code,
        authorName: body.author_name,
        offerAction: body.offer_action === true,
      },
    };
  }
  if (typeof (body as { status?: unknown }).status === "string") {
    return { kind: "limited", status: (body as { status: string }).status };
  }
  return { kind: "unavailable" };
}

/** Render the loaded view with text-only insertion. */
export function renderPublicRequest(container: HTMLElement, view: PublicView): void {
  container.textContent = "";
  if (view.kind === "unavailable") {
    const message = document.createElement("p");
    message.textContent = "This request is unavailable.";
    container.appendChild(message);
    return;
  }
  if (view.kind === "limited") {
    const message = document.createElement("p");
    message.textContent = `This request is ${view.status} and takes no new offers.`;
    container.appendChild(message);
    return;
  }
  const title = document.createElement("h1");
  title.textContent = view.detail.title;
  container.appendChild(title);
  const facts = document.createElement("p");
  facts.textContent = `Budget ${view.detail.budget} — ${view.detail.condition} — ${view.detail.regionLabel}, ${view.detail.city}`;
  container.appendChild(facts);
  if (view.detail.offerAction) {
    const action = document.createElement("a");
    action.href = `/r/${view.detail.id}#offer`;
    action.textContent = "I have this";
    container.appendChild(action);
  }
}
