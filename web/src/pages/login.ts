/**
 * Phone login: real registration, proof, and session flows with explicit
 * local-provider limitation. Proof states stay distinct (pending, wrong,
 * expired, rate-limited, error); a wrong or expired proof never
 * authenticates and never authorizes a write. After success, a preserved
 * shared-request intent returns to its current resource, whose
 * availability the next screen rechecks server-side.
 *
 * No fake-success shortcut exists here: every transition answers from
 * the backend response. The live phone provider is a documented later
 * gate; the disposable fake provider behind these routes is test-only.
 */
import { ApiClient, ContractError, createMutationIntent } from "../api/client.js";
import { renderErrorSummary } from "../components/error-summary.js";
import { createActionButton, setPending } from "../components/action-button.js";
import { createFormField, setFieldError } from "../components/form-field.js";
import { requestPath } from "../navigation/router.js";
import { SessionStore } from "../state/session.js";

export type LoginState =
  | "idle"
  | "pending"
  | "wrong"
  | "expired"
  | "limited"
  | "error"
  | "active";

export interface RegistrationFields {
  displayName: string;
  phone: string;
  city: string;
  region: string;
}

interface AccountReceipt {
  account_id?: unknown;
}

/** Register the minimal pending account; one key per intended account. */
export async function registerAccount(
  client: ApiClient,
  fields: RegistrationFields,
): Promise<string> {
  const body = await client.mutate<{ account_id?: unknown }>(
    "POST",
    "/api/v1/accounts",
    {
      display_name: fields.displayName,
      phone: fields.phone,
      city: fields.city,
      region: fields.region,
      policy_version: "v1",
    },
    createMutationIntent(),
  );
  if (typeof (body as AccountReceipt).account_id !== "string") {
    throw new ContractError("invalid_field", 500, "registration receipt carries no account");
  }
  return (body as unknown as { account_id: string }).account_id;
}

/** Request a proof challenge for a registered phone. */
export async function requestChallenge(client: ApiClient, phone: string): Promise<void> {
  await client.mutate("POST", "/api/v1/accounts/challenges", { phone }, createMutationIntent());
}

/** Confirm proof and open a session; maps proof failures to states. */
export async function confirmProof(
  client: ApiClient,
  session: SessionStore,
  phone: string,
  code: string,
  displayName: string,
): Promise<LoginState> {
  try {
    await client.mutate(
      "POST",
      "/api/v1/accounts/challenges/confirmations",
      { phone, code },
      createMutationIntent(),
    );
  } catch (error) {
    if (error instanceof ContractError) {
      if (error.code === "verification_failed") return "wrong";
      if (error.code === "expired") return "expired";
      if (error.code === "rate_limited") return "limited";
    }
    return "error";
  }
  try {
    const login = await client.mutate<{ account_state?: unknown }>(
      "POST",
      "/api/v1/sessions",
      { phone, code },
      createMutationIntent(),
    );
    if ((login as { account_state?: unknown }).account_state !== "active") {
      return "error";
    }
  } catch {
    return "error";
  }
  const current = await client.get<{ account_id?: unknown }>("/api/v1/sessions/current");
  if (typeof (current as { account_id?: unknown }).account_id !== "string") {
    return "error";
  }
  session.startSession(
    (current as unknown as { account_id: string }).account_id,
    displayName,
  );
  return session.isAuthenticated() ? "active" : "error";
}

/**
 * Submit one proof code: the same mapping the rendered form uses.
 * Returns the resulting state; only "active" holds a session.
 */
export async function submitCode(
  client: ApiClient,
  session: SessionStore,
  phone: string,
  code: string,
  displayName: string,
): Promise<LoginState> {
  return confirmProof(client, session, phone, code, displayName);
}

/** Where to continue after login: preserved intent or home. */
export function continuationPath(session: SessionStore): string {
  const intent = session.takePendingIntent();
  if (intent !== undefined) {
    return requestPath(intent.requestId);
  }
  return "/";
}

export interface LoginView {
  root: HTMLElement;
  submitRegistration: (fields: RegistrationFields) => Promise<LoginState>;
  submitCode: (phone: string, code: string, displayName: string) => Promise<LoginState>;
}

/**
 * Render the login flow: registration form, then proof form, with one
 * error region. Buttons guard while pending; server errors surface as
 * text and never as a session.
 */
export function renderLogin(
  container: HTMLElement,
  client: ApiClient,
  session: SessionStore,
): LoginView {
  container.textContent = "";
  const summary = document.createElement("div");
  container.appendChild(summary);
  const form = document.createElement("form");
  form.noValidate = true;
  container.appendChild(form);

  const nameField = createFormField({ id: "login-name", label: "Display name" });
  const phoneField = createFormField({ id: "login-phone", label: "Phone", type: "tel" });
  const cityField = createFormField({ id: "login-city", label: "City" });
  const regionField = createFormField({ id: "login-region", label: "Region" });
  const codeField = createFormField({ id: "login-code", label: "Code", type: "text" });
  codeField.hidden = true;
  const submit = createActionButton({ label: "Create account", pendingLabel: "Working…" });
  form.append(nameField, phoneField, cityField, regionField, codeField, submit);

  const read = (wrapper: HTMLElement): string =>
    wrapper.querySelector("input")?.value.trim() ?? "";

  function explain(state: LoginState): void {
    const messages: Record<LoginState, string> = {
      idle: "",
      pending: "Code sent. Enter it to continue.",
      wrong: "Wrong code. Check and try again.",
      expired: "Code expired. Register again for a fresh code.",
      limited: "Too many attempts. Wait before retrying.",
      error: "Something went wrong. Nothing was signed in.",
      active: "",
    };
    renderErrorSummary(summary, messages[state] === "" ? [] : [messages[state] as string]);
  }

  let armedPhone = "";
  let armedName = "";

  async function submitRegistration(fields: RegistrationFields): Promise<LoginState> {
    setPending(submit, true);
    try {
      await registerAccount(client, fields);
      await requestChallenge(client, fields.phone);
    } catch (error) {
      if (error instanceof ContractError && error.code === "duplicate_intent") {
        renderErrorSummary(summary, ["This phone is already registered. Use account recovery."]);
        return "error";
      }
      explain("error");
      return "error";
    } finally {
      setPending(submit, false);
    }
    armedPhone = fields.phone;
    armedName = fields.displayName;
    codeField.hidden = false;
    submit.textContent = "Confirm code";
    explain("pending");
    return "pending";
  }

  async function submitCodeForm(phone: string, code: string, displayName: string): Promise<LoginState> {
    setPending(submit, true);
    try {
      const state = await submitCode(client, session, phone, code, displayName);
      if (state === "active") {
        armedPhone = "";
        armedName = "";
      } else {
        explain(state);
      }
      return state;
    } finally {
      setPending(submit, false);
    }
  }

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    if (codeField.hidden) {
      const fields = {
        displayName: read(nameField),
        phone: read(phoneField),
        city: read(cityField),
        region: read(regionField),
      };
      if (fields.displayName === "") setFieldError(nameField, "Display name is required.");
      if (fields.phone === "") setFieldError(phoneField, "Phone is required.");
      void submitRegistration(fields);
    } else {
      const code = read(codeField);
      if (code === "") {
        setFieldError(codeField, "Code is required.");
        return;
      }
      void submitCodeForm(armedPhone, code, armedName);
    }
  });

  return { root: container, submitRegistration, submitCode: submitCodeForm };
}
