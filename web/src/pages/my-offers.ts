/**
 * Seller's own offers: status and terms history for the signed-in
 * seller only. Competing offers, buyer data, and cross-seller material
 * never appear: the endpoints themselves are seller-scoped and every
 * refusals stays a refusal. Withdrawal and edit endpoints do not exist
 * in this delivery, so no such buttons are offered here.
 */
import { ApiClient, ContractError } from "../api/client.js";

export interface OwnOffer {
  id: string;
  requestId: string;
  description: string;
  price: string;
  condition: string;
  state: string;
}

interface OfferRow {
  id?: unknown;
  request_id?: unknown;
  description?: unknown;
  price?: unknown;
  condition?: unknown;
  state?: unknown;
}

function readOffer(row: OfferRow): OwnOffer | null {
  if (
    typeof row.id !== "string" ||
    typeof row.request_id !== "string" ||
    typeof row.description !== "string" ||
    typeof row.price !== "string" ||
    typeof row.condition !== "string" ||
    typeof row.state !== "string"
  ) {
    return null;
  }
  return {
    id: row.id,
    requestId: row.request_id,
    description: row.description,
    price: row.price,
    condition: row.condition,
    state: row.state,
  };
}

/** List the signed-in seller's own offers, newest first. */
export async function fetchMyOffers(client: ApiClient): Promise<OwnOffer[]> {
  const body = await client.get<unknown>("/api/v1/offers/mine");
  const rows = Array.isArray(body) ? body : (body as { offers?: unknown }).offers;
  if (!Array.isArray(rows)) {
    throw new ContractError("invalid_field", 500, "own offers malformed");
  }
  const items: OwnOffer[] = [];
  for (const row of rows) {
    const offer = readOffer(row as OfferRow);
    if (offer !== null) {
      items.push(offer);
    }
  }
  return items;
}

/** Load one owned offer with its current standing. */
export async function fetchOwnOffer(client: ApiClient, offerId: string): Promise<OwnOffer> {
  const body = await client.get<OfferRow>(`/api/v1/offers/${offerId}`);
  const offer = readOffer(body);
  if (offer === null) {
    throw new ContractError("invalid_field", 500, "offer detail malformed");
  }
  return offer;
}

/** Render the seller's own offers with standing per slot. */
export function renderMyOffers(container: HTMLElement, items: OwnOffer[]): void {
  container.textContent = "";
  if (items.length === 0) {
    const empty = document.createElement("p");
    empty.textContent = "No offers sent yet.";
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
    facts.textContent = `${item.condition}, ${item.state}`;
    row.appendChild(facts);
    list.appendChild(row);
  }
  container.appendChild(list);
}
