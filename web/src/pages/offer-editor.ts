/**
 * Seller offer editor: validated submission against the live demand.
 * Item, price, condition, and availability travel with the observed
 * cycle and revision; wrong conditions, above-budget prices, and stale
 * demands refuse with named errors before and after the write. No
 * edit, view, decline, or withdrawal endpoint exists in this delivery,
 * so none is offered here (asserted absent in e2e).
 */
import { ApiClient, ContractError, createMutationIntent } from "../api/client.js";
import { renderErrorSummary } from "../components/error-summary.js";
import { createActionButton, setPending } from "../components/action-button.js";
import { createFormField } from "../components/form-field.js";

export interface OfferInput {
  description: string;
  price: string;
  condition: string;
  city: string;
  region: string;
  available: boolean;
}

const BUDGET_PATTERN = /^[0-9]+(\.[0-9]{1,2})?$/;

/** Validate offer input against the submission grammar. */
export function validateOfferInput(raw: Partial<OfferInput>): OfferInput {
  const fail = (message: string): never => {
    throw new ContractError("invalid_field", 400, message);
  };
  const description = (raw.description ?? "").trim();
  if (description === "" || description.length > 120) {
    fail("Description needs 1 to 120 characters.");
  }
  const price = (raw.price ?? "").trim();
  if (!BUDGET_PATTERN.test(price) || Number(price) <= 0) {
    fail("Price must be a positive amount with at most two decimals.");
  }
  const condition = raw.condition ?? "";
  if (condition !== "new" && condition !== "used") {
    fail("Offers carry new or used only.");
  }
  const city = (raw.city ?? "").trim();
  const region = (raw.region ?? "").trim();
  if (city === "" || region === "") fail("Seller locality is required.");
  return { description, price, condition, city, region, available: raw.available !== false };
}

export interface DemandStanding {
  id: string;
  cycle: number;
  revision: number;
  state: string;
  budget: string;
}

/** Fetch the live demand standing a submission is composed against. */
export async function fetchDemandStanding(
  client: ApiClient,
  requestId: string,
): Promise<DemandStanding> {
  const body = await client.get<{
    id?: unknown;
    cycle_number?: unknown;
    revision_number?: unknown;
    state?: unknown;
    budget?: unknown;
  }>(`/api/v1/requests/${requestId}`);
  if (
    typeof body.id !== "string" ||
    typeof body.cycle_number !== "number" ||
    typeof body.revision_number !== "number" ||
    typeof body.state !== "string" ||
    typeof body.budget !== "string"
  ) {
    throw new ContractError("invalid_field", 500, "demand standing malformed");
  }
  return {
    id: body.id,
    cycle: body.cycle_number,
    revision: body.revision_number,
    state: body.state,
    budget: body.budget,
  };
}

/** Submit one offer slot against the observed cycle and revision. */
export async function submitOffer(
  client: ApiClient,
  standing: DemandStanding,
  input: OfferInput,
): Promise<string> {
  const body = await client.mutate<{ id?: unknown }>(
    "POST",
    `/api/v1/requests/${standing.id}/offers`,
    {
      cycle_number: standing.cycle,
      revision_number: standing.revision,
      description: input.description,
      price: input.price,
      condition: input.condition,
      city_code: input.city,
      region_code: input.region,
      available: input.available,
      available_in_city: true,
    },
    createMutationIntent(`rev-${standing.revision}`),
  );
  if (typeof body.id !== "string") {
    throw new ContractError("invalid_field", 500, "offer receipt carries no id");
  }
  return body.id;
}

/** Render the offer editor with validation and named server errors. */
export function renderOfferEditor(
  container: HTMLElement,
  client: ApiClient,
  standing: DemandStanding,
  onSubmitted?: (offerId: string) => void,
): void {
  container.textContent = "";
  const summary = document.createElement("div");
  container.appendChild(summary);
  if (standing.state !== "active") {
    renderErrorSummary(summary, ["This demand takes no new offers."]);
    return;
  }
  const form = document.createElement("form");
  form.noValidate = true;
  container.appendChild(form);
  const descriptionField = createFormField({ id: "offer-description", label: "Item description" });
  const priceField = createFormField({ id: "offer-price", label: "Price" });
  const conditionField = createFormField({ id: "offer-condition", label: "Condition (new or used)" });
  const cityField = createFormField({ id: "offer-city", label: "City" });
  const regionField = createFormField({ id: "offer-region", label: "Region" });
  const submit = createActionButton({ label: "Send offer", pendingLabel: "Sending…" });
  form.append(descriptionField, priceField, conditionField, cityField, regionField, submit);

  const read = (wrapper: HTMLElement): string =>
    wrapper.querySelector("input")?.value.trim() ?? "";

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    let input: OfferInput;
    try {
      input = validateOfferInput({
        description: read(descriptionField),
        price: read(priceField),
        condition: read(conditionField),
        city: read(cityField),
        region: read(regionField),
        available: true,
      });
    } catch (error) {
      renderErrorSummary(summary, [
        error instanceof ContractError ? error.message : "Check the highlighted fields.",
      ]);
      return;
    }
    setPending(submit, true);
    submitOffer(client, standing, input).then(
      (offerId) => {
        setPending(submit, false);
        renderErrorSummary(summary, []);
        onSubmitted?.(offerId);
      },
      (error: unknown) => {
        setPending(submit, false);
        renderErrorSummary(summary, [
          error instanceof ContractError
            ? `${error.code}: ${error.message} Nothing was sent.`
            : "Could not send the offer.",
        ]);
      },
    );
  });
}
