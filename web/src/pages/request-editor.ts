/**
 * Request draft editor: validated creation, careful editing, honest
 * publication. Client validation mirrors the server grammar (titles,
 * exact money, known locality) so mistakes surface before any write;
 * server refusals render as field errors with no false success.
 * Material requirement changes gate behind an explicit confirmation
 * naming the offer-retirement consequence; the revision endpoint that
 * retires live offers is a documented pending surface, so confirmation
 * text never claims a retirement this call performs on its own.
 */
import { ApiClient, ContractError, createMutationIntent } from "../api/client.js";
import { renderErrorSummary } from "../components/error-summary.js";
import { createActionButton, setPending } from "../components/action-button.js";
import { createFormField, setFieldError } from "../components/form-field.js";

export interface DraftInput {
  title: string;
  category: string;
  budget: string;
  condition: string;
  city: string;
  region: string;
  notes: string;
}

export interface LocalityVocabulary {
  cities: string[];
  regionsByCity: Record<string, string[]>;
  categories: string[];
}

interface CatalogCity {
  code: string;
  regions?: Array<{ code: string }>;
}

interface CatalogCategory {
  code: string;
}

/** Fetch live locality and category vocabulary for validation. */
export async function fetchVocabulary(client: ApiClient): Promise<LocalityVocabulary> {
  const [citiesBody, categoriesBody] = await Promise.all([
    client.get<{ cities?: CatalogCity[] }>("/api/v1/catalogs/cities"),
    client.get<{ categories?: CatalogCategory[] }>("/api/v1/catalogs/categories"),
  ]);
  const regionsByCity: Record<string, string[]> = {};
  const cities: string[] = [];
  for (const city of citiesBody.cities ?? []) {
    cities.push(city.code);
    regionsByCity[city.code] = (city.regions ?? []).map((region) => region.code);
  }
  return {
    cities,
    regionsByCity,
    categories: (categoriesBody.categories ?? []).map((category) => category.code),
  };
}

const BUDGET_PATTERN = /^[0-9]+(\.[0-9]{1,2})?$/;

/** Validate draft input against live vocabulary and server grammar. */
export function validateDraftInput(
  raw: Partial<DraftInput>,
  vocabulary: LocalityVocabulary,
): DraftInput {
  const fail = (message: string): never => {
    throw new ContractError("invalid_field", 400, message);
  };
  const title = (raw.title ?? "").trim();
  if (title === "" || title.length > 120) fail("Title needs 1 to 120 characters.");
  if (raw.category === undefined || !vocabulary.categories.includes(raw.category)) {
    fail("Choose a known category.");
  }
  const budget = (raw.budget ?? "").trim();
  if (!BUDGET_PATTERN.test(budget) || Number(budget) <= 0) {
    fail("Budget must be a positive amount with at most two decimals.");
  }
  if (raw.condition !== "new" && raw.condition !== "used" && raw.condition !== "either") {
    fail("Choose a known condition.");
  }
  if (raw.city === undefined || !vocabulary.cities.includes(raw.city)) {
    fail("Choose a known city.");
  }
  const regions = vocabulary.regionsByCity[raw.city as string] ?? [];
  if (raw.region === undefined || !regions.includes(raw.region)) {
    fail("Choose a region of the selected city.");
  }
  const notes = raw.notes ?? "";
  if (notes.length > 500) fail("Notes hold at most 500 characters.");
  return {
    title,
    category: raw.category as string,
    budget,
    condition: raw.condition as string,
    city: raw.city as string,
    region: raw.region as string,
    notes,
  };
}

/** Requirement fields whose change is material to live offers. */
export const MATERIAL_FIELDS = ["title", "category", "budget", "condition", "city", "region"] as const;

/** Which material fields differ between two requirement sets. */
export function materialChanges(current: DraftInput, next: DraftInput): string[] {
  return MATERIAL_FIELDS.filter((field) => current[field] !== next[field]);
}

/**
 * Consequence notice shown before a material save when live offers
 * exist: names the retired count without performing any retirement.
 */
export function retirementNotice(liveOffers: number, changed: string[]): string {
  if (liveOffers <= 0 || changed.length === 0) {
    return "";
  }
  return (
    `Changing ${changed.join(", ")} retires ${liveOffers} live ` +
    `offer${liveOffers === 1 ? "" : "s"}. Confirm to continue with the draft save; ` +
    `live-offer retirement itself lands with the revision flow.`
  );
}

/** Create one private draft through the real endpoint. */
export async function createDraft(client: ApiClient, input: DraftInput): Promise<string> {
  const body = await client.mutate<{ id?: unknown }>(
    "POST",
    "/api/v1/requests/drafts",
    {
      title: input.title,
      category_code: input.category,
      budget: input.budget,
      condition: input.condition,
      city_code: input.city,
      region_code: input.region,
      notes: input.notes,
    },
    createMutationIntent(),
  );
  if (typeof body.id !== "string") {
    throw new ContractError("invalid_field", 500, "draft receipt carries no id");
  }
  return body.id;
}

/** Replace draft requirements while the demand is still a draft. */
export async function editDraft(
  client: ApiClient,
  draftId: string,
  input: DraftInput,
): Promise<void> {
  await client.mutate(
    "PUT",
    `/api/v1/requests/drafts/${draftId}`,
    {
      title: input.title,
      category_code: input.category,
      budget: input.budget,
      condition: input.condition,
      city_code: input.city,
      region_code: input.region,
      notes: input.notes,
    },
    createMutationIntent(),
  );
}

/** Publish one draft into its first seven-day cycle. */
export async function publishDraft(client: ApiClient, draftId: string): Promise<string> {
  const body = await client.mutate<{ id?: unknown }>(
    "POST",
    `/api/v1/requests/drafts/${draftId}/publication`,
    {},
    createMutationIntent(),
  );
  if (typeof body.id !== "string") {
    throw new ContractError("invalid_field", 500, "publication receipt carries no id");
  }
  return body.id;
}

export interface EditorOptions {
  initial?: Partial<DraftInput>;
  liveOffers?: number;
  current?: DraftInput;
  onSaved?: (draftId: string) => void;
}

/** Render the draft editor with validation, consequence gate, and save. */
export function renderDraftEditor(
  container: HTMLElement,
  client: ApiClient,
  vocabulary: LocalityVocabulary,
  options?: EditorOptions,
): void {
  container.textContent = "";
  const summary = document.createElement("div");
  container.appendChild(summary);
  const form = document.createElement("form");
  form.noValidate = true;
  container.appendChild(form);
  const initial = options?.initial ?? {};
  function field(id: string, label: string, value?: string): HTMLElement {
    if (value === undefined) {
      return createFormField({ id, label });
    }
    return createFormField({ id, label, value });
  }
  const titleField = field("draft-title", "Title", initial.title);
  const categoryField = field("draft-category", "Category", initial.category);
  const budgetField = field("draft-budget", "Budget", initial.budget);
  const conditionField = field("draft-condition", "Condition", initial.condition);
  const cityField = field("draft-city", "City", initial.city);
  const regionField = field("draft-region", "Region", initial.region);
  const notesField = field("draft-notes", "Notes", initial.notes);
  const confirmBox = document.createElement("label");
  const confirmInput = document.createElement("input");
  confirmInput.type = "checkbox";
  confirmInput.id = "draft-confirm";
  const confirmText = document.createElement("span");
  confirmText.textContent = "I understand live offers retire on material change.";
  confirmBox.append(confirmInput, confirmText);
  confirmBox.hidden = true;
  const save = createActionButton({ label: "Save draft", pendingLabel: "Saving…" });
  form.append(
    titleField,
    categoryField,
    budgetField,
    conditionField,
    cityField,
    regionField,
    notesField,
    confirmBox,
    save,
  );

  const read = (wrapper: HTMLElement): string =>
    wrapper.querySelector("input")?.value.trim() ?? "";

  function collect(): DraftInput {
    return validateDraftInput(
      {
        title: read(titleField),
        category: read(categoryField),
        budget: read(budgetField),
        condition: read(conditionField),
        city: read(cityField),
        region: read(regionField),
        notes: read(notesField),
      },
      vocabulary,
    );
  }

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    let input: DraftInput;
    try {
      input = collect();
    } catch (error) {
      renderErrorSummary(summary, [
        error instanceof ContractError ? error.message : "Check the highlighted fields.",
      ]);
      return;
    }
    const changed =
      options?.current !== undefined ? materialChanges(options.current, input) : [];
    const notice = retirementNotice(options?.liveOffers ?? 0, changed);
    if (notice !== "") {
      renderErrorSummary(summary, [notice]);
      confirmBox.hidden = false;
      if (!confirmInput.checked) {
        return;
      }
    }
    setPending(save, true);
    createDraft(client, input).then(
      (draftId) => {
        setPending(save, false);
        renderErrorSummary(summary, []);
        options?.onSaved?.(draftId);
      },
      (error: unknown) => {
        setPending(save, false);
        if (error instanceof ContractError && error.fields !== undefined) {
          const names = Object.keys(error.fields);
          if (names.length > 0) {
            renderErrorSummary(summary, [
              `Server refused: ${names.join(", ")}. Nothing was saved.`,
            ]);
            return;
          }
        }
        renderErrorSummary(summary, [
          error instanceof ContractError ? `${error.message} Nothing was saved.` : "Could not save.",
        ]);
      },
    );
  });
}
