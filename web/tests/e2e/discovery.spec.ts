/**
 * Discovery e2e (P16-T01) against the real disposable stack: anonymous
 * understanding, filter discipline, and no protected content for blocked
 * viewers or suspended links. The browser drives the built bundle over
 * HTTP; fixtures travel the real public flows; nothing is mocked.
 */
import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { e2eBaseUrl, requireDisposableDb, registerAccount } from "../support/stack.js";

const FAKE_CODE = "135790";

test.describe.configure({ mode: "serial" });

function fixtures(): { suspended_demand?: string } {
  const path = process.env["E2E_FIXTURES_PATH"] ?? "/tmp/procurali-e2e-fixtures.json";
  try {
    return JSON.parse(readFileSync(path, "utf8")) as { suspended_demand?: string };
  } catch {
    return {};
  }
}

function phone(): string {
  const tail = String(1000 + Math.floor(Math.random() * 9000)).padStart(4, "0");
  return `+55 11 90000-${tail}`;
}

async function postJson(
  base: string,
  path: string,
  body: unknown,
  cookie?: string,
): Promise<{ status: number; json: unknown; cookie: string }> {
  const headers: Record<string, string> = { "content-type": "application/json", origin: base };
  if (cookie !== undefined) headers["cookie"] = cookie;
  const response = await fetch(`${base}${path}`, {
    method: "POST",
    headers,
    body: JSON.stringify(body),
  });
  const text = await response.text();
  return {
    status: response.status,
    json: text === "" ? null : (JSON.parse(text) as unknown),
    cookie: response.headers.get("set-cookie") ?? "",
  };
}

async function publishDemand(base: string, cookie: string, title: string): Promise<string> {
  const draft = await postJson(base, "/api/v1/requests/drafts", {
    title,
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, cookie);
  expect(draft.status).toBe(201);
  const demandId = (draft.json as { id: string }).id;
  const published = await postJson(base, `/api/v1/requests/drafts/${demandId}/publication`, {}, cookie);
  expect([200, 201]).toContain(published.status);
  return (published.json as { id?: string }).id ?? demandId;
}

test.beforeAll(() => {
  requireDisposableDb();
  e2eBaseUrl();
  process.env["PROCURALI_E2E_FAKE_CODE"] = FAKE_CODE;
});

test("anonymous active request is understandable before signup", async ({ page }) => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const demandId = await publishDemand(base, buyer.cookie, "E2E Refrigerator");

  await page.goto(`${base}/assets/request-template.html`);
  const view = await page.evaluate(async (args: { base: string; id: string }) => {
    const pages = await import("/assets/pages/public-request.js") as typeof import(
      "../../src/pages/public-request.js"
    );
    const api = await import("/assets/api/client.js") as typeof import(
      "../../src/api/client.js"
    );
    const reader = new api.ApiClient({ baseUrl: args.base });
    const loaded = await pages.loadPublicRequest(reader, args.id);
    const host = document.createElement("div");
    document.body.appendChild(host);
    pages.renderPublicRequest(host, loaded);
    return { kind: loaded.kind, text: host.textContent ?? "" };
  }, { base, id: demandId });
  expect(view.kind).toBe("full");
  await expect(page.getByRole("heading", { name: "E2E Refrigerator" })).toBeVisible();
  await expect(page.getByText("600.00")).toBeVisible();
  await expect(page.getByRole("link", { name: "I have this" })).toBeVisible();
  expect(view.text).not.toMatch(/\d{8,}/);
});

test("filters respect city, positive bounds, and current visibility", async ({ page }) => {
  const base = e2eBaseUrl();
  const suspended = fixtures().suspended_demand;
  expect(suspended).toBeDefined();
  await page.goto(`${base}/assets/request-template.html`);
  const outcome = await page.evaluate(async (args: { base: string; suspended: string }) => {
    const discovery = await import("/assets/pages/discovery.js") as typeof import(
      "../../src/pages/discovery.js"
    );
    const api = await import("/assets/api/client.js") as typeof import(
      "../../src/api/client.js"
    );
    const reader = new api.ApiClient({ baseUrl: args.base });
    const vocabulary = await discovery.fetchCatalogs(reader);
    const refused: string[] = [];
    for (const raw of [
      { city: "atlantis" },
      { city: "campinas", maxBudget: "-5" },
      { city: "campinas", maxBudget: "10.000" },
      { city: "campinas", condition: "refurbished" },
      { city: "" },
    ]) {
      try {
        discovery.validateFilters(raw, vocabulary);
        refused.push("accepted");
      } catch (error) {
        refused.push(error instanceof api.ContractError ? error.code : "threw");
      }
    }
    const valid = discovery.validateFilters(
      { city: "campinas", category: "home_appliances", maxBudget: "600.00", condition: "either" },
      vocabulary,
    );
    const suspendedCard = await discovery.resolveRequestCard(reader, args.suspended);
    return { cities: vocabulary.cities, refused, valid, suspendedCard };
  }, { base, suspended: suspended ?? "" });
  expect(outcome.cities).toContain("campinas");
  expect(outcome.refused).toEqual([
    "invalid_field",
    "invalid_field",
    "invalid_field",
    "invalid_field",
    "invalid_field",
  ]);
  expect(outcome.valid.city).toBe("campinas");
  expect(outcome.suspendedCard).toBeNull();
});

test("blocked viewer and suspended link show no protected content", async ({ page, context }) => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Block Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E Block Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const who = await fetch(`${base}/api/v1/sessions/current`, {
    headers: { cookie: seller.cookie },
  });
  expect(who.status).toBe(200);
  const sellerId = ((await who.json()) as { account_id?: string }).account_id ?? "";
  expect(sellerId).not.toBe("");
  const demandId = await publishDemand(base, buyer.cookie, "E2E Blocked Fridge");
  const blocked = await postJson(base, "/api/v1/blocks", { blocked_user_id: sellerId }, buyer.cookie);
  expect(blocked.status).toBe(201);

  const token = seller.cookie.split(";")[0]?.split("=").slice(1).join("=") ?? "";
  await context.addCookies([{ name: "session", value: token, domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${base}/assets/request-template.html`);
  const view = await page.evaluate(async (args: { base: string; id: string }) => {
    const pages = await import("/assets/pages/public-request.js") as typeof import(
      "../../src/pages/public-request.js"
    );
    const api = await import("/assets/api/client.js") as typeof import(
      "../../src/api/client.js"
    );
    const reader = new api.ApiClient({ baseUrl: args.base });
    const loaded = await pages.loadPublicRequest(reader, args.id);
    const host = document.createElement("div");
    document.body.appendChild(host);
    pages.renderPublicRequest(host, loaded);
    return { kind: loaded.kind, text: host.textContent ?? "" };
  }, { base, id: demandId });
  expect(view.kind).toBe("unavailable");
  await expect(page.getByText("This request is unavailable.")).toBeVisible();
  await expect(page.getByRole("link", { name: "I have this" })).toHaveCount(0);
  expect(view.text).not.toContain("E2E Blocked Fridge");
  expect(view.text).not.toMatch(/\d{8,}/);
});
