/**
 * Discovery screen: city-first filters over real catalog vocabulary and
 * safe public request cards. There is deliberately no anonymous listing
 * endpoint in this delivery: cards resolve through the public detail
 * route (one demand at a time, server-projected), so filters gate what
 * the visitor opens rather than inventing a result set. Every refusal
 * names its reason before any network mutation; nothing here creates
 * offers, contacts, events, or quota consumption.
 */
import { ApiClient, ContractError } from "../api/client.js";

export interface DiscoveryFilters {
  city: string;
  category?: string;
  maxBudget?: string;
  condition?: string;
}

export interface CatalogVocabulary {
  cities: string[];
  categories: string[];
}

interface CatalogItem {
  code: string;
}

async function getCodes(client: ApiClient, path: string): Promise<string[]> {
  const body = await client.get<{
    items?: CatalogItem[];
    cities?: CatalogItem[];
    categories?: CatalogItem[];
  }>(path);
  const items = body.items ?? body.cities ?? body.categories ?? [];
  return items.map((item) => item.code);
}

/** Fetch live filter vocabulary: cities and categories. */
export async function fetchCatalogs(client: ApiClient): Promise<CatalogVocabulary> {
  const [cities, categories] = await Promise.all([
    getCodes(client, "/api/v1/catalogs/cities"),
    getCodes(client, "/api/v1/catalogs/categories"),
  ]);
  return { cities, categories };
}

/**
 * Validate raw filter input against live vocabulary and positive bounds.
 * Unknown cities/categories, non-positive budgets, and unknown
 * conditions refuse locally with `invalid_field` and send nothing.
 */
export function validateFilters(
  raw: { city?: string; category?: string; maxBudget?: string; condition?: string },
  vocabulary: CatalogVocabulary,
): DiscoveryFilters {
  const city = (raw.city ?? "").trim();
  if (city === "" || !vocabulary.cities.includes(city)) {
    throw new ContractError("invalid_field", 400, "unknown city");
  }
  let category: string | undefined;
  if (raw.category !== undefined && raw.category !== "") {
    if (!vocabulary.categories.includes(raw.category)) {
      throw new ContractError("invalid_field", 400, "unknown category");
    }
    category = raw.category;
  }
  let maxBudget: string | undefined;
  if (raw.maxBudget !== undefined && raw.maxBudget !== "") {
    if (!/^[0-9]+(\.[0-9]{1,2})?$/.test(raw.maxBudget) || Number(raw.maxBudget) <= 0) {
      throw new ContractError("invalid_field", 400, "budget must be a positive decimal");
    }
    maxBudget = raw.maxBudget;
  }
  let condition: string | undefined;
  if (raw.condition !== undefined && raw.condition !== "") {
    if (raw.condition !== "new" && raw.condition !== "used" && raw.condition !== "either") {
      throw new ContractError("invalid_field", 400, "unknown condition");
    }
    condition = raw.condition;
  }
  const filters: DiscoveryFilters = { city };
  if (category !== undefined) filters.category = category;
  if (maxBudget !== undefined) filters.maxBudget = maxBudget;
  if (condition !== undefined) filters.condition = condition;
  return filters;
}

/** One safe public card: allowlisted fields only, never offers or phones. */
export interface DiscoveryCard {
  id: string;
  title: string;
  budget: string;
  condition: string;
  city: string;
  regionLabel: string;
}

interface DetailBody {
  id?: unknown;
  title?: unknown;
  budget_cents?: unknown;
  condition?: unknown;
  city_code?: unknown;
  region_code?: unknown;
}

function formatBudget(cents: number): string {
  return `${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, "0")}`;
}

/**
 * Resolve one demand through the public detail route. Returns the safe
 * card when the server projects full details, or null when the demand
 * is unavailable to this viewer (terminal, hidden, suspended,
 * prohibited, blocked, or missing) — with no protected content read.
 */
export async function resolveRequestCard(
  client: ApiClient,
  requestId: string,
): Promise<DiscoveryCard | null> {
  let body: DetailBody;
  try {
    body = await client.get<DetailBody>(`/api/v1/public/requests/${requestId}`);
  } catch (error) {
    if (error instanceof ContractError && error.code === "not_found") {
      return null;
    }
    throw error;
  }
  if (
    typeof body.id !== "string" ||
    typeof body.title !== "string" ||
    typeof body.budget_cents !== "number" ||
    typeof body.condition !== "string" ||
    typeof body.city_code !== "string" ||
    typeof body.region_code !== "string"
  ) {
    return null;
  }
  return {
    id: body.id,
    title: body.title,
    budget: formatBudget(body.budget_cents),
    condition: body.condition,
    city: body.city_code,
    regionLabel: body.region_code,
  };
}

/** Render cards in server order; an empty set reads as empty, never an error. */
export function renderDiscovery(container: HTMLElement, cards: DiscoveryCard[]): void {
  container.textContent = "";
  if (cards.length === 0) {
    const empty = document.createElement("p");
    empty.textContent = "No requests match these filters yet.";
    container.appendChild(empty);
    return;
  }
  const list = document.createElement("ul");
  for (const card of cards) {
    const item = document.createElement("li");
    const link = document.createElement("a");
    link.href = `/r/${card.id}`;
    link.textContent = `${card.title} — ${card.budget} — ${card.regionLabel}`;
    item.appendChild(link);
    list.appendChild(item);
  }
  container.appendChild(list);
}
