/**
 * Professional profile: free declaration with factual display, plus
 * account-deletion consequences without a deletion action. Declaring
 * changes no capability and withdrawing keeps history; both facts
 * render exactly as recorded. Reputation renders the modest factual
 * projection only — no score, badge, sale count, or complaint count
 * exists anywhere here. Account deletion has no endpoint in this
 * delivery, so consequences show as information with no button.
 */
import { ApiClient, ContractError, createMutationIntent } from "../api/client.js";
import { renderErrorSummary } from "../components/error-summary.js";
import { createActionButton, setPending } from "../components/action-button.js";
import { createFormField } from "../components/form-field.js";

export const BUSINESS_TYPES = ["shop", "merchant", "reseller", "business", "recurring"] as const;

export interface ProfessionalDeclaration {
  businessName: string;
  businessType: string;
  city: string;
  region: string;
}

/** Declare (or redeclare) the free professional classification. */
export async function declareProfessional(
  client: ApiClient,
  input: ProfessionalDeclaration,
): Promise<void> {
  if (!(BUSINESS_TYPES as readonly string[]).includes(input.businessType)) {
    throw new ContractError("invalid_field", 400, "unknown business type");
  }
  await client.mutate(
    "PUT",
    "/api/v1/profiles/professional",
    {
      business_name: input.businessName,
      business_type: input.businessType,
      city: input.city,
      region: input.region,
    },
    createMutationIntent(),
  );
}

/** Withdraw the declaration; the row and its history survive. */
export async function withdrawProfessional(client: ApiClient): Promise<void> {
  await client.mutate(
    "DELETE",
    "/api/v1/profiles/me/professional",
    {},
    createMutationIntent(),
  );
}

export interface ReputationFacts {
  accountAgeDays: number;
  activeRecently: boolean;
  resolutionsReported: number | null;
}

/** Load the modest factual projection for one account. */
export async function fetchReputation(
  client: ApiClient,
  userId: string,
): Promise<ReputationFacts> {
  const body = await client.get<{
    account_age_days?: unknown;
    active_recently?: unknown;
    resolutions_reported?: unknown;
  }>(`/api/v1/profiles/${userId}/reputation`);
  if (
    typeof body.account_age_days !== "number" ||
    typeof body.active_recently !== "boolean" ||
    (body.resolutions_reported !== undefined &&
      body.resolutions_reported !== null &&
      typeof body.resolutions_reported !== "number")
  ) {
    throw new ContractError("invalid_field", 500, "reputation projection malformed");
  }
  return {
    accountAgeDays: body.account_age_days,
    activeRecently: body.active_recently,
    resolutionsReported:
      typeof body.resolutions_reported === "number" ? body.resolutions_reported : null,
  };
}

/** Deletion consequences, shown as information (no endpoint, no button). */
export const DELETION_CONSEQUENCES =
  "Deleting the account closes new contact access. History needed for safety review stays retained.";

/** Render declaration state, reputation facts, and deletion information. */
export function renderProfile(
  container: HTMLElement,
  client: ApiClient,
  reputation: ReputationFacts,
): void {
  container.textContent = "";
  const summary = document.createElement("div");
  container.appendChild(summary);
  const form = document.createElement("form");
  form.noValidate = true;
  container.appendChild(form);
  const nameField = createFormField({ id: "pro-name", label: "Business name" });
  const typeField = createFormField({ id: "pro-type", label: "Business type" });
  const cityField = createFormField({ id: "pro-city", label: "City" });
  const regionField = createFormField({ id: "pro-region", label: "Region" });
  const save = createActionButton({ label: "Declare professional", pendingLabel: "Saving…" });
  form.append(nameField, cityField, regionField, typeField, save);
  const read = (wrapper: HTMLElement): string =>
    wrapper.querySelector("input")?.value.trim() ?? "";
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    setPending(save, true);
    declareProfessional(client, {
      businessName: read(nameField),
      businessType: read(typeField),
      city: read(cityField),
      region: read(regionField),
    }).then(
      () => {
        setPending(save, false);
        renderErrorSummary(summary, [
          "Declaration recorded. It is free and changes no capability.",
        ]);
      },
      (error: unknown) => {
        setPending(save, false);
        renderErrorSummary(summary, [
          error instanceof ContractError ? error.message : "Could not declare.",
        ]);
      },
    );
  });
  const facts = document.createElement("dl");
  const age = document.createElement("dt");
  age.textContent = "Account age (days)";
  const ageValue = document.createElement("dd");
  ageValue.textContent = String(reputation.accountAgeDays);
  const active = document.createElement("dt");
  active.textContent = "Active recently";
  const activeValue = document.createElement("dd");
  activeValue.textContent = reputation.activeRecently ? "yes" : "no";
  facts.append(age, ageValue, active, activeValue);
  if (reputation.resolutionsReported !== null) {
    const resolutions = document.createElement("dt");
    resolutions.textContent = "Buyer-reported resolutions";
    const resolutionsValue = document.createElement("dd");
    resolutionsValue.textContent = String(reputation.resolutionsReported);
    facts.append(resolutions, resolutionsValue);
  }
  container.appendChild(facts);
  const consequences = document.createElement("p");
  consequences.textContent = DELETION_CONSEQUENCES;
  container.appendChild(consequences);
}
