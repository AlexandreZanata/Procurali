/**
 * Auth e2e (P16-T02) against the real disposable stack: proof states,
 * shared-request continuation, profile locality, and safe phone
 * conflicts. Nothing is mocked except the documented expired-branch
 * mapping check, which carries the exact backend 410 shape (expired
 * challenges are proven end to end by the backend auth suite; the fake
 * proof code below is test-only input for the disposable provider).
 */
import { test, expect } from "@playwright/test";
import { e2eBaseUrl, requireDisposableDb, registerAccount } from "../support/stack.js";

const FAKE_CODE = "135790";

test.describe.configure({ mode: "serial" });

function phone(): string {
  const tail = String(1000 + Math.floor(Math.random() * 9000)).padStart(4, "0");
  return `+55 11 90000-${tail}`;
}

async function postJson(
  base: string,
  path: string,
  body: unknown,
  cookie?: string,
): Promise<{ status: number; json: unknown }> {
  const headers: Record<string, string> = { "content-type": "application/json", origin: base };
  if (cookie !== undefined) headers["cookie"] = cookie;
  const response = await fetch(`${base}${path}`, {
    method: "POST",
    headers,
    body: JSON.stringify(body),
  });
  const text = await response.text();
  return { status: response.status, json: text === "" ? null : (JSON.parse(text) as unknown) };
}

test.beforeAll(() => {
  requireDisposableDb();
  e2eBaseUrl();
  process.env["PROCURALI_E2E_FAKE_CODE"] = FAKE_CODE;
});

test("wrong proof cannot authenticate and anonymous writes stay refused", async ({ page }) => {
  const base = e2eBaseUrl();
  await page.goto(`${base}/assets/request-template.html`);
  const outcome = await page.evaluate(async (args: { base: string }) => {
    const login = await import("/assets/pages/login.js") as {
      renderLogin: (
        container: HTMLElement,
        client: { mutate: (...a: never[]) => Promise<unknown>; get: (...a: never[]) => Promise<unknown> },
        session: {
          isAuthenticated: () => boolean;
          startSession: (u: string, d: string) => void;
        },
      ) => {
        submitRegistration: (fields: {
          displayName: string;
          phone: string;
          city: string;
          region: string;
        }) => Promise<string>;
        submitCode: (phone: string, code: string, name: string) => Promise<string>;
      };
    };
    const api = await import("/assets/api/client.js") as {
      ApiClient: new (options: { baseUrl: string }) => {
        mutate: (...a: never[]) => Promise<unknown>;
        get: (...a: never[]) => Promise<unknown>;
      };
      ContractError: new (...a: never[]) => { status: number };
    };
    const state = await import("/assets/state/session.js") as {
      SessionStore: new () => {
        isAuthenticated: () => boolean;
        startSession: (u: string, d: string) => void;
      };
    };
    const reader = new api.ApiClient({ baseUrl: args.base });
    const store = new state.SessionStore();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = login.renderLogin(host, reader as never, store as never);
    const name = `E2E Wrong ${Math.floor(Math.random() * 9000)}`;
    const phoneNumber = `+55 11 9000${Math.floor(1000 + Math.random() * 9000)}`;
    const registered = await view.submitRegistration({
      displayName: name,
      phone: phoneNumber,
      city: "Campinas",
      region: "SP",
    });
    const wrong = await view.submitCode(phoneNumber, "000000", name);
    const summary = host.textContent ?? "";
    let anonymousStatus = 0;
    try {
      await reader.mutate("POST", "/api/v1/requests/drafts", { title: "Nope" }, { key: "anon-1" });
    } catch (error) {
      if (error instanceof api.ContractError) anonymousStatus = error.status;
    }
    return { registered, wrong, summary, authenticated: store.isAuthenticated(), anonymousStatus };
  }, { base });
  expect(outcome.registered).toBe("pending");
  expect(outcome.wrong).toBe("wrong");
  expect(outcome.summary).toContain("Wrong code");
  expect(outcome.authenticated).toBe(false);
  expect(outcome.anonymousStatus).toBe(401);
});

test("expired proof maps to its own state without a session", async ({ page }) => {
  const base = e2eBaseUrl();
  await page.goto(`${base}/assets/request-template.html`);
  const outcome = await page.evaluate(async () => {
    const login = await import("/assets/pages/login.js") as {
      submitCode: (
        client: unknown,
        session: { isAuthenticated: () => boolean },
        phone: string,
        code: string,
        name: string,
      ) => Promise<string>;
    };
    const state = await import("/assets/state/session.js") as {
      SessionStore: new () => { isAuthenticated: () => boolean };
    };
    // Stubbed transport carrying the exact backend 410 shape (a real
    // ContractError with the backend's code and status); the live
    // 5-minute expiry itself is proven by the backend auth suite.
    const api = await import("/assets/api/client.js");
    const stubClient = {
      mutate: async () => {
        throw new api.ContractError("expired", 410, "challenge expired");
      },
      get: async () => null,
    };
    const store = new state.SessionStore();
    const appError = await login.submitCode(
      stubClient,
      store,
      "+55 11 90000-0000",
      "135790",
      "Nobody",
    );
    return { appError, authenticated: store.isAuthenticated() };
  });
  expect(outcome.appError).toBe("expired");
  expect(outcome.authenticated).toBe(false);
});

test("login from a shared request returns to the current valid context", async ({ page }) => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E continuity Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E continuity Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const created = await postJson(base, "/api/v1/requests/drafts", {
    title: "E2E continuity Fridge",
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  expect(created.status).toBe(201);
  const demandId = (created.json as { id: string }).id;
  await postJson(base, `/api/v1/requests/drafts/${demandId}/publication`, {}, buyer.cookie);
  const offered = await postJson(base, `/api/v1/requests/${demandId}/offers`, {
    cycle_number: 1,
    revision_number: 1,
    description: "continuity 300L",
    price: "520.00",
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  }, seller.cookie);
  expect(offered.status).toBe(201);

  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async (args: { requestId: string }) => {
    const state = await import("/assets/state/session.js");
    const store = new state.SessionStore();
    store.saveIntentForLogin({ requestId: args.requestId, intentKey: { key: "shared-1" } });
    (window as unknown as { __store: unknown }).__store = store;
  }, { requestId: demandId });

  await page.evaluate(async () => {
    const login = await import("/assets/pages/login.js");
    const api = await import("/assets/api/client.js");
    const state = await import("/assets/state/session.js");
    const store = (window as unknown as { __store: state.SessionStore }).__store;
    const client = new api.ApiClient({ baseUrl: window.location.origin });
    const host = document.createElement("div");
    document.body.appendChild(host);
    login.renderLogin(host, client, store);
  });
  const newcomerPhone = phone();
  const newcomerName = `E2E Newcomer ${newcomerPhone.slice(-4)}`;
  await page.getByLabel("Display name").fill(newcomerName);
  await page.getByLabel("Phone").fill(newcomerPhone);
  await page.getByLabel("City").fill("Campinas");
  await page.getByLabel("Region").fill("SP");
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.getByText("Code sent. Enter it to continue.")).toBeVisible();
  await page.getByLabel("Code").fill(FAKE_CODE);
  await page.getByRole("button", { name: "Confirm code" }).click();
  await expect.poll(async () => {
    return page.evaluate(() => {
      const store = (window as unknown as { __store: { isAuthenticated: () => boolean } }).__store;
      return store.isAuthenticated();
    });
  }).toBe(true);

  const continuation = await page.evaluate(async (args: { base: string }) => {
    const login = await import("/assets/pages/login.js");
    const api = await import("/assets/api/client.js");
    const pages = await import("/assets/pages/public-request.js");
    const store = (window as unknown as { __store: never }).__store as Parameters<
      typeof login.continuationPath
    >[0];
    const path = login.continuationPath(store);
    const reader = new api.ApiClient({ baseUrl: args.base });
    const id = path.split("/r/")[1] ?? "";
    const loaded = await pages.loadPublicRequest(reader, id);
    return { path, kind: loaded.kind };
  }, { base });
  expect(continuation.path).toBe(`/r/${demandId}`);
  expect(continuation.kind).toBe("full");
});

test("profile move keeps request locality and phone conflicts refuse safely", async ({ page }) => {
  const base = e2eBaseUrl();
  const buyerPhone = phone();
  const buyer = await registerAccount(base, {
    displayName: "E2E Local Buyer",
    phone: buyerPhone,
    city: "Campinas",
    region: "SP",
  });
  const created = await postJson(base, "/api/v1/requests/drafts", {
    title: "E2E Local Fridge",
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  const demandId = (created.json as { id: string }).id;
  await postJson(base, `/api/v1/requests/drafts/${demandId}/publication`, {}, buyer.cookie);

  // Profile move to another city: the published demand keeps Campinas.
  const patched = await fetch(`${base}/api/v1/accounts/me`, {
    method: "PATCH",
    headers: { "content-type": "application/json", origin: base, cookie: buyer.cookie },
    body: JSON.stringify({ city: "Valinhos", region: "Centro" }),
  });
  expect(patched.status).toBe(200);
  const detail = await fetch(`${base}/api/v1/requests/${demandId}`, {
    headers: { cookie: buyer.cookie },
  });
  expect(detail.status).toBe(200);
  const body = (await detail.json()) as { city_code?: string };
  expect(body.city_code).toBe("campinas");

  // Phone conflict: the same number refuses with a recovery offer and
  // never disturbs the first session.
  const clash = await postJson(base, "/api/v1/accounts", {
    display_name: "E2E Clash",
    phone: buyerPhone,
    city: "Campinas",
    region: "SP",
    policy_version: "v1",
  });
  expect(clash.status).toBe(409);
  expect(JSON.stringify(clash.json)).toContain("recovery");
  const still = await fetch(`${base}/api/v1/sessions/current`, {
    headers: { cookie: buyer.cookie },
  });
  expect(still.status).toBe(200);

  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async () => {
    const account = await import("/assets/pages/account.js");
    const host = document.createElement("div");
    document.body.appendChild(host);
    account.renderAccount(host, null as never, null as never, {
      accountId: "00000000-0000-0000-0000-000000000000",
      displayName: "Reader",
      city: "Valinhos",
      region: "Centro",
      state: "active",
    });
  });
  await expect(page.getByText("Changing your default city never moves already published requests.")).toBeVisible();
});
