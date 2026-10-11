/**
 * Buyer comparison: received offers for one owned demand, newest first.
 * Terms render with their revision standing so updates stay visible;
 * sellers stay anonymous beyond their display names and no phone,
 * competing-seller, or cross-demand material appears. Decline, edit,
 * and withdrawal endpoints do not exist in this delivery, so no such
 * buttons are offered here.
 */
import { ApiClient, ContractError } from "../api/client.js";

export interface ReceivedOffer {
  id: string;
  description: string;
  price: string;
  condition: string;
  state: string;
  revision: number;
  sellerName: string;
}

interface OfferRow {
  id?: unknown;
  description?: unknown;
  price?: unknown;
  condition?: unknown;
  state?: unknown;
  revision_number?: unknown;
  seller_name?: unknown;
}

function readOffer(row: OfferRow): ReceivedOffer | null {
  const revision = typeof row.revision_number === "number" ? row.revision_number : null;
  const sellerName = typeof row.seller_name === "string" ? row.seller_name : null;
  if (
    typeof row.id !== "string" ||
    typeof row.description !== "string" ||
    typeof row.price !== "string" ||
    typeof row.condition !== "string" ||
    typeof row.state !== "string" ||
    revision === null ||
    sellerName === null
  ) {
    return null;
  }
  return {
    id: row.id,
    description: row.description,
    price: row.price,
    condition: row.condition,
    state: row.state,
    revision,
    sellerName,
  };
}

/** List the buyer's received offers for one owned demand. */
export async function fetchReceivedOffers(
  client: ApiClient,
  requestId: string,
): Promise<ReceivedOffer[]> {
  const body = await client.get<{ live?: unknown; history?: unknown }>(
    `/api/v1/requests/${requestId}/offers`,
  );
  const rows = [
    ...(Array.isArray(body.live) ? body.live : []),
    ...(Array.isArray(body.history) ? body.history : []),
  ];
  const items: ReceivedOffer[] = [];
  for (const row of rows) {
    const offer = readOffer(row as OfferRow);
    if (offer !== null) {
      items.push(offer);
    }
  }
  return items;
}

/** Render the comparison list with revision standing per offer. */
export function renderReceivedOffers(container: HTMLElement, items: ReceivedOffer[]): void {
  container.textContent = "";
  if (items.length === 0) {
    const empty = document.createElement("p");
    empty.textContent = "No offers yet.";
    container.appendChild(empty);
    return;
  }
  const list = document.createElement("ul");
  for (const item of items) {
    const row = document.createElement("li");
    const heading = document.createElement("strong");
    heading.textContent = `${item.description} — ${item.price}`;
    row.appendChild(heading);
    const facts = document.createElement("p");
    facts.textContent =
      `${item.condition}, ${item.state}, terms revision ${item.revision}, ` +
      `from ${item.sellerName}`;
    row.appendChild(facts);
    list.appendChild(row);
  }
  container.appendChild(list);
  if (items.some((item) => item.revision > 1)) {
    const note = document.createElement("p");
    note.textContent = "Some offers carry updated terms.";
    container.appendChild(note);
  }
}
