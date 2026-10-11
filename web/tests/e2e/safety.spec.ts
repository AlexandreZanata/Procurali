/**
 * Safety e2e (P16-T06) against the real disposable stack: safe sharing,
 * block-gated contact, terminal-state reporting, free professional
 * offers, and factual reputation. Feedback submission and account
 * deletion have no endpoints in this delivery, so the suite asserts
 * honest information display (and no fake buttons) instead of
 * inventing the calls.
 */
import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { e2eBaseUrl, requireDisposableDb, registerAccount } from "../support/stack.js";

test.describe.configure({ mode: "serial" });

function phone(): string {
  const tail = String(1000 + Math.floor(Math.random() * 9000)).padStart(4, "0");
  return `+55 11 90000-${tail}`;
}

function fixtures(): { withdrawn_offer?: string } {
  const path = process.env["E2E_FIXTURES_PATH"] ?? "/tmp/procurali-e2e-fixtures.json";
  try {
    return JSON.parse(readFileSync(path, "utf8")) as { withdrawn_offer?: string };
  } catch {
    return {};
  }
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
  const id = (draft.json as { id: string }).id;
  const published = await postJson(base, `/api/v1/requests/drafts/${id}/publication`, {}, cookie);
  expect([200, 201]).toContain(published.status);
  return id;
}

async function loginAs(base: string, phoneNumber: string): Promise<string> {
  const response = await fetch(`${base}/api/v1/sessions`, {
    method: "POST",
    headers: { "content-type": "application/json", origin: base },
    body: JSON.stringify({ phone: phoneNumber, code: "135790" }),
  });
  expect(response.status).toBe(200);
  return (response.headers.get("set-cookie") ?? "").split(";")[0] ?? "";
}

test.beforeAll(() => {
  requireDisposableDb();
  e2eBaseUrl();
  process.env["PROCURALI_E2E_FAKE_CODE"] = "135790";
});

test("share carries public context and unpublished needs refuse", async ({ page, context }) => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Share Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const demandId = await publishDemand(base, buyer.cookie, "E2E Shareable Fridge");
  const token = buyer.cookie.split(";")[0]?.split("=").slice(1).join("=") ?? "";
  await context.addCookies([{ name: "session", value: token, domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${base}/assets/request-template.html`);
  const share = await page.evaluate(async (args: { base: string; id: string }) => {
    const action = await import("/assets/pages/share-action.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    const host = document.createElement("div");
    document.body.appendChild(host);
    const prepared = await new Promise<unknown>((resolve, reject) => {
      action.renderShareAction(host, client, args.id, (receipt) => resolve(receipt));
      const buttons = [...host.querySelectorAll("button")];
      const copy = buttons.find((button) => button.textContent === "Copy link");
      copy?.click();
      setTimeout(() => reject(new Error("share timed out")), 15000);
    });
    return prepared as { link: string; message: string; channel: string };
  }, { base, id: demandId });
  expect(share.channel).toBe("copy");
  expect(share.link).toContain(demandId);
  expect(share.message).toContain("E2E Shareable Fridge");
  expect(share.message).not.toMatch(/\d{8,}/);
  await expect(page.getByText("E2E Shareable Fridge", { exact: false })).toBeVisible();

  // An unpublished draft refuses sharing with nothing recorded.
  const draft = await postJson(base, "/api/v1/requests/drafts", {
    title: "E2E Draft Fridge",
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  const draftId = (draft.json as { id: string }).id;
  const refused = await postJson(base, `/api/v1/public/requests/${draftId}/share`, { channel: "copy" }, buyer.cookie);
  expect(refused.status).not.toBe(200);
});

test("block stops contact, unblock restores, withdrawn stays reportable", async ({ page, context }) => {
  // Slow-auth environment (documented linear session scan on the
  // disposable stack): more time, same assertions.
  test.slow();
  const base = e2eBaseUrl();
  const seed = fixtures();
  expect(seed.withdrawn_offer).toBeDefined();
  const buyer = await registerAccount(base, {
    displayName: "E2E Guard Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E Guard Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const demandId = await publishDemand(base, buyer.cookie, "E2E Guard Fridge");
  const demand2 = await publishDemand(base, buyer.cookie, "E2E Guard Fridge Two");
  async function offerOn(demand: string, description: string): Promise<string> {
    const offered = await postJson(base, `/api/v1/requests/${demand}/offers`, {
      cycle_number: 1,
      revision_number: 1,
      description,
      price: "520.00",
      condition: "used",
      city_code: "campinas",
      region_code: "centro",
      available: true,
      available_in_city: true,
    }, seller.cookie);
    expect(offered.status).toBe(201);
    return (offered.json as { id: string }).id;
  }
  const offerId = await offerOn(demandId, "Guard 300L");
  const offer2 = await offerOn(demand2, "Guard 280L");

  async function contact(demand: string, offer: string, cookie: string): Promise<number> {
    const response = await postJson(
      base,
      `/api/v1/requests/${demand}/offers/${offer}/contact`,
      { handoff_id: crypto.randomUUID(), expected_offer_terms: 1, entry_source: "offer_detail" },
      cookie,
    );
    return response.status;
  }
  expect([200, 201]).toContain(await contact(demandId, offerId, buyer.cookie));

  const who = (await (await fetch(`${base}/api/v1/sessions/current`, {
    headers: { cookie: seller.cookie },
  })).json()) as { account_id?: string };
  expect(who.account_id).toBeDefined();
  expect(await (await postJson(base, "/api/v1/blocks", { blocked_user_id: who.account_id ?? "" }, buyer.cookie)).status).toBe(201);

  // A fresh slot contacted only after the block refuses with the
  // named state (no replay confounding: this pair never shook hands).
  const stopped = await contact(demand2, offer2, buyer.cookie);
  expect(stopped).toBe(409);

  // Unblock restores handoffs through the same real endpoint.
  const token = buyer.cookie.split(";")[0]?.split("=").slice(1).join("=") ?? "";
  await context.addCookies([{ name: "session", value: token, domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${base}/assets/request-template.html`);
  const restored = await page.evaluate(async (args: { base: string; seller: string }) => {
    const safety = await import("/assets/pages/safety-actions.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    await safety.unblockUser(client, args.seller);
    return true;
  }, { base, seller: who.account_id ?? "" });
  expect(restored).toBe(true);
  // Old offers stay unavailable by design, so restoration is proven on
  // fresh supply submitted after the unblock.
  const demand3 = await publishDemand(base, buyer.cookie, "E2E Guard Fridge Three");
  const offered3 = await postJson(base, `/api/v1/requests/${demand3}/offers`, {
    cycle_number: 1,
    revision_number: 1,
    description: "Guard 260L",
    price: "470.00",
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  }, seller.cookie);
  expect(offered3.status).toBe(201);
  const offer3 = (offered3.json as { id: string }).id;
  const again = await contact(demand3, offer3, buyer.cookie);
  expect([200, 201]).toContain(again);

  // Withdrawn-terminal supply stays reportable through its real route.
  const harnessCookie = await loginAs(base, "+55 11 90000-3101");
  const reported = await postJson(base, "/api/v1/reports", {
    target_kind: "offer",
    target_id: seed.withdrawn_offer ?? "",
    reason: "spam",
    detail: "terminal report from e2e",
  }, harnessCookie);
  expect(reported.status).toBe(201);
});

test("free professional offers work with factual reputation", async ({ page, context }) => {
  // Slow-auth environment (documented linear session scan on the
  // disposable stack): more time, same assertions.
  test.slow();
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Pro Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E Pro Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const demandId = await publishDemand(base, buyer.cookie, "E2E Pro Fridge");
  const token = seller.cookie.split(";")[0]?.split("=").slice(1).join("=") ?? "";
  await context.addCookies([{ name: "session", value: token, domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async () => {
    const profile = await import("/assets/pages/profile.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({
      baseUrl: window.location.origin,
      fetchImpl: fetch.bind(window),
    });
    const host = document.createElement("div");
    document.body.appendChild(host);
    profile.renderProfile(host, client, { accountAgeDays: 0, activeRecently: false, resolutionsReported: null });
  });
  await page.getByLabel("Business name").fill("E2E Corner Shop");
  await page.getByLabel("Business type").fill("shop");
  await page.getByLabel("City").fill("Campinas");
  await page.getByLabel("Region").fill("SP");
  await page.getByRole("button", { name: "Declare professional" }).click();
  // Slow-auth environment (documented linear session scan): same text,
  // longer wait.
  await expect(page.getByText("Declaration recorded. It is free and changes no capability.")).toBeVisible({
    timeout: 60000,
  });

  // Declared professionals keep ordinary free capabilities.
  const offered = await postJson(base, `/api/v1/requests/${demandId}/offers`, {
    cycle_number: 1,
    revision_number: 1,
    description: "Pro 300L",
    price: "520.00",
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  }, seller.cookie);
  expect(offered.status).toBe(201);

  const who = (await (await fetch(`${base}/api/v1/sessions/current`, {
    headers: { cookie: seller.cookie },
  })).json()) as { account_id?: string };
  const reputation = await page.evaluate(async (args: { base: string; id: string }) => {
    const profile = await import("/assets/pages/profile.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    const facts = await profile.fetchReputation(client, args.id);
    document.body.textContent = "";
    const host = document.createElement("div");
    document.body.appendChild(host);
    profile.renderProfile(host, client, facts);
    return host.textContent ?? "";
  }, { base, id: who.account_id ?? "" });
  expect(reputation).toContain("Account age (days)");
  for (const absent of ["trust score", "verified sales", "complaint", "badge", "stars"]) {
    expect(reputation.toLowerCase()).not.toContain(absent);
  }
  await expect(page.getByText(/Deleting the account closes new contact access/)).toBeVisible();
  const buttons = await page.getByRole("button").allTextContents();
  for (const forbidden of ["Delete account", "Disable account"]) {
    expect(buttons).not.toContain(forbidden);
  }
});
