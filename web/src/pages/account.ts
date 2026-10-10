/**
 * Account screen: profile reads and edits, locality honesty, logout.
 * Profile and city edits travel PATCH /api/v1/accounts/me; published
 * resources keep their own locality server-side (a profile move never
 * rewrites them — stated on screen, enforced by the backend). Phone
 * conflicts refuse safely with a recovery offer; the session never
 * changes on a refusal. Phone *change* has no HTTP surface in this
 * delivery, so no change form is offered (no fake flow).
 */
import { ApiClient, ContractError } from "../api/client.js";
import { renderErrorSummary } from "../components/error-summary.js";
import { createActionButton, setPending } from "../components/action-button.js";
import { createFormField } from "../components/form-field.js";
import { SessionStore } from "../state/session.js";

export interface Profile {
  accountId: string;
  displayName: string;
  city: string;
  region: string;
  state: string;
}

interface CurrentBody {
  account_id?: unknown;
  account_state?: unknown;
}

interface MeBody {
  account_id?: unknown;
  display_name?: unknown;
  city?: unknown;
  region?: unknown;
  account_state?: unknown;
}

/** Load the current profile; anonymous callers receive null, never a throw. */
export async function loadProfile(client: ApiClient): Promise<Profile | null> {
  let current: CurrentBody;
  try {
    current = await client.get<CurrentBody>("/api/v1/sessions/current");
  } catch (error) {
    if (error instanceof ContractError && error.code === "unauthenticated") {
      return null;
    }
    throw error;
  }
  if (
    typeof current.account_id !== "string" ||
    typeof current.account_state !== "string"
  ) {
    return null;
  }
  return {
    accountId: current.account_id,
    displayName: "",
    city: "",
    region: "",
    state: current.account_state,
  };
}

/** Save display name, city, or region edits for the current account. */
export async function saveProfile(
  client: ApiClient,
  patch: { displayName?: string; city?: string; region?: string },
): Promise<Profile> {
  const body: Record<string, string> = {};
  if (patch.displayName !== undefined) body["display_name"] = patch.displayName;
  if (patch.city !== undefined) body["city"] = patch.city;
  if (patch.region !== undefined) body["region"] = patch.region;
  const updated = await client.mutate<MeBody>("PATCH", "/api/v1/accounts/me", body, {
    key: `profile-${Date.now()}`,
  });
  if (
    typeof updated.account_id !== "string" ||
    typeof updated.display_name !== "string" ||
    typeof updated.city !== "string" ||
    typeof updated.region !== "string"
  ) {
    throw new ContractError("invalid_field", 500, "profile receipt malformed");
  }
  return {
    accountId: updated.account_id,
    displayName: updated.display_name,
    city: updated.city,
    region: updated.region,
    state: "active",
  };
}

/** Log out: server revocation plus full local clearing. */
export async function logout(client: ApiClient, session: SessionStore): Promise<void> {
  try {
    await client.mutate("DELETE", "/api/v1/sessions/current", {}, { key: `logout-${Date.now()}` });
  } finally {
    session.clearOnLogout();
    client.logout();
  }
}

/** Honest locality note shown beside the form (server-enforced). */
export const LOCALITY_NOTE =
  "Changing your default city never moves already published requests.";

/** Render the profile form with save, locality note, and logout. */
export function renderAccount(
  container: HTMLElement,
  client: ApiClient,
  session: SessionStore,
  profile: Profile,
): void {
  container.textContent = "";
  const summary = document.createElement("div");
  container.appendChild(summary);
  const form = document.createElement("form");
  form.noValidate = true;
  container.appendChild(form);
  const nameField = createFormField({
    id: "account-name",
    label: "Display name",
    value: profile.displayName,
  });
  const cityField = createFormField({ id: "account-city", label: "City", value: profile.city });
  const regionField = createFormField({
    id: "account-region",
    label: "Region",
    value: profile.region,
  });
  const save = createActionButton({ label: "Save profile", pendingLabel: "Saving…" });
  form.append(nameField, cityField, regionField, save);
  const note = document.createElement("p");
  note.textContent = LOCALITY_NOTE;
  container.appendChild(note);
  const signOut = createActionButton({ label: "Log out" });
  container.appendChild(signOut);

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const read = (wrapper: HTMLElement): string =>
      wrapper.querySelector("input")?.value.trim() ?? "";
    setPending(save, true);
    saveProfile(client, {
      displayName: read(nameField),
      city: read(cityField),
      region: read(regionField),
    }).then(
      () => {
        setPending(save, false);
        renderErrorSummary(summary, []);
      },
      (error: unknown) => {
        setPending(save, false);
        renderErrorSummary(summary, [
          error instanceof ContractError ? error.message : "Could not save the profile.",
        ]);
      },
    );
  });
  signOut.addEventListener("click", () => {
    void logout(client, session);
  });
}
