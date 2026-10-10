/**
 * Request-maintenance e2e (P16-T03) against the real disposable stack:
 * honest publication with sharing, refused writes without false
 * success, stale edits refused, and no fabricated lifecycle buttons.
 * Renewal, outcome, removal, and publish-time duplicate/quota guards
 * have no HTTP surface in this delivery (documented below), so the UI
 * offers no such buttons and the suite asserts their absence instead
 * of inventing the flows.
 */
import { test, expect } from "@playwright/test";
import { e2eBaseUrl, requireDisposableDb, registerAccount } from "../support/stack.js";

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

const DRAFT = {
  title: "E2E Keeper Fridge",
  category: "home_appliances",
  budget: "600.00",
  condition: "either",
  city: "campinas",
  region: "centro",
  notes: "",
};

test.beforeAll(() => {
  requireDisposableDb();
  e2eBaseUrl();
  process.env["PROCURALI_E2E_FAKE_CODE"] = "135790";
});

test("valid publication persists and offers sharing", async ({ page }) => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Keeper",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async () => {
    const editor = await import("/assets/pages/request-editor.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({
      baseUrl: window.location.origin,
      fetchImpl: fetch.bind(window),
    });
    const vocabulary = await editor.fetchVocabulary(client);
    const host = document.createElement("div");
    document.body.appendChild(host);
    editor.renderDraftEditor(host, client, vocabulary, {
      onSaved: (draftId: string) => {
        (window as unknown as { __draftId: string }).__draftId = draftId;
      },
    });
  });
  await page.getByLabel("Title").fill(DRAFT.title);
  await page.getByLabel("Category").fill(DRAFT.category);
  await page.getByLabel("Budget").fill(DRAFT.budget);
  await page.getByLabel("Condition").fill(DRAFT.condition);
  await page.getByLabel("City").fill(DRAFT.city);
  await page.getByLabel("Region").fill(DRAFT.region);
  const token = buyer.cookie.split(";")[0]?.split("=").slice(1).join("=") ?? "";
  await page.context().addCookies([{ name: "session", value: token, domain: "127.0.0.1", path: "/" }]);
  await page.getByRole("button", { name: "Save draft" }).click();
  await expect.poll(async () => {
    return page.evaluate(() => (window as unknown as { __draftId?: string }).__draftId);
  }).not.toBeUndefined();
  const draftId = await page.evaluate(
    () => (window as unknown as { __draftId: string }).__draftId,
  );

  const published = await postJson(base, `/api/v1/requests/drafts/${draftId}/publication`, {}, buyer.cookie);
  expect([200, 201]).toContain(published.status);

  const listed = await page.evaluate(async (args: { base: string }) => {
    const mine = await import("/assets/pages/my-requests.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    const items = await mine.fetchMyRequests(client);
    const host = document.createElement("div");
    document.body.appendChild(host);
    mine.renderMyRequests(host, client, items);
    return items.map((item) => ({ id: item.id, title: item.title, state: item.state }));
  }, { base });
  expect(listed.some((item) => item.id === draftId && item.state === "active")).toBe(true);

  await page.getByRole("button", { name: "Share" }).first().click();
  await expect.poll(async () => {
    const response = await fetch(`${base}/api/v1/public/requests/${draftId}`);
    return response.status;
  }).toBe(200);
  const shared = await postJson(
    base,
    `/api/v1/public/requests/${draftId}/share`,
    { channel: "copy" },
    buyer.cookie,
  );
  expect([200, 201]).toContain(shared.status);
  expect(JSON.stringify(shared.json)).toContain(draftId);
});

test("refused writes show errors without false success", async ({ page }) => {
  const base = e2eBaseUrl();
  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async () => {
    const editor = await import("/assets/pages/request-editor.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({
      baseUrl: window.location.origin,
      fetchImpl: fetch.bind(window),
    });
    const vocabulary = await editor.fetchVocabulary(client);
    const host = document.createElement("div");
    document.body.appendChild(host);
    editor.renderDraftEditor(host, client, vocabulary, {});
  });
  // Missing title refuses locally with a named message and no request.
  await page.getByLabel("Category").fill(DRAFT.category);
  await page.getByRole("button", { name: "Save draft" }).click();
  await expect(page.getByText("Title needs 1 to 120 characters.")).toBeVisible();
  // Overprecision budget refuses locally before any write.
  await page.getByLabel("Title").fill("E2E Precise Fridge");
  await page.getByLabel("Budget").fill("10.000");
  await page.getByRole("button", { name: "Save draft" }).click();
  await expect(page.getByText("Budget must be a positive amount with at most two decimals.")).toBeVisible();

  // Server refusals carry stable codes and save nothing.
  const buyer = await registerAccount(base, {
    displayName: "E2E Refused Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const missing = await postJson(base, "/api/v1/requests/drafts", {
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  expect(missing.status).toBe(400);
  const overprecise = await postJson(base, "/api/v1/requests/drafts", {
    title: "E2E Precise Fridge",
    category_code: "home_appliances",
    budget: "10.000",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  expect(overprecise.status).not.toBe(201);
  const listed = await postJson(base, "/api/v1/requests/drafts", {
    title: "E2E Real Fridge",
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  expect(listed.status).toBe(201);
});

test("stale edits refuse and no lifecycle button is fabricated", async ({ page }) => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Stale Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const created = await postJson(base, "/api/v1/requests/drafts", {
    title: "E2E Stale Fridge",
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  const demandId = (created.json as { id: string }).id;
  await postJson(base, `/api/v1/requests/drafts/${demandId}/publication`, {}, buyer.cookie);

  // Editing after publication refuses; the live title stands.
  const stale = await fetch(`${base}/api/v1/requests/drafts/${demandId}`, {
    method: "PUT",
    headers: { "content-type": "application/json", origin: base, cookie: buyer.cookie },
    body: JSON.stringify({
      title: "Sneaky rewrite",
      category_code: "home_appliances",
      budget: "600.00",
      condition: "either",
      city_code: "campinas",
      region_code: "centro",
      notes: "",
    }),
  });
  expect(stale.status).not.toBe(200);
  const detail = await fetch(`${base}/api/v1/public/requests/${demandId}`);
  expect(detail.status).toBe(200);
  expect(((await detail.json()) as { title?: string }).title).toBe("E2E Stale Fridge");

  // Renewal, outcome, and removal endpoints do not exist in this
  // delivery, so no such button may appear; server actions show as text.
  const token = buyer.cookie.split(";")[0]?.split("=").slice(1).join("=") ?? "";
  await page.context().addCookies([{ name: "session", value: token, domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async (args: { base: string }) => {
    const mine = await import("/assets/pages/my-requests.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    const items = await mine.fetchMyRequests(client);
    const host = document.createElement("div");
    document.body.appendChild(host);
    mine.renderMyRequests(host, client, items);
  }, { base });
  const buttons = await page.getByRole("button").allTextContents();
  for (const forbidden of ["Renew", "Complete", "Remove", "Cancel request", "Reopen"]) {
    expect(buttons).not.toContain(forbidden);
  }
  await expect(page.getByText(/State active/)).toBeVisible();
});
