/**
 * Staff e2e (P16-T07) against the real disposable stack: scoped grants,
 * reasoned decisions with observable effects, and no destructive
 * shortcuts. Ban, reversal, suspension, restoration, and policy
 * endpoints do not exist in this delivery; the suite asserts their
 * absence (including forged calls) instead of inventing them.
 */
import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { e2eBaseUrl, requireDisposableDb, registerAccount } from "../support/stack.js";

test.describe.configure({ mode: "serial" });

function phone(): string {
  const tail = String(1000 + Math.floor(Math.random() * 9000)).padStart(4, "0");
  return `+55 11 90000-${tail}`;
}

function fixtures(): { suspended_demand?: string } {
  const path = process.env["E2E_FIXTURES_PATH"] ?? "/tmp/procurali-e2e-fixtures.json";
  try {
    return JSON.parse(readFileSync(path, "utf8")) as { suspended_demand?: string };
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

async function loginAs(base: string, phoneNumber: string): Promise<string> {
  const response = await fetch(`${base}/api/v1/sessions`, {
    method: "POST",
    headers: { "content-type": "application/json", origin: base },
    body: JSON.stringify({ phone: phoneNumber, code: "135790" }),
  });
  expect(response.status).toBe(200);
  return (response.headers.get("set-cookie") ?? "").split(";")[0] ?? "";
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

function sessionToken(cookie: string): string {
  return cookie.split(";")[0]?.split("=").slice(1).join("=") ?? "";
}

test.beforeAll(() => {
  requireDisposableDb();
  e2eBaseUrl();
  process.env["PROCURALI_E2E_FAKE_CODE"] = "135790";
});

test("moderator has no ban or reversal path, forged or otherwise", async ({ page, context }) => {
  // Slow-auth environment (documented linear session scan on the
  // disposable stack): more time, same assertions.
  test.slow();
  const base = e2eBaseUrl();
  const moderatorCookie = await loginAs(base, "+55 11 90000-3104");
  await context.addCookies([{ name: "session", value: sessionToken(moderatorCookie), domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async (args: { base: string }) => {
    const cases = await import("/assets/pages/staff-cases.js");
    const actions = await import("/assets/pages/staff-actions.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    const queueHost = document.createElement("div");
    document.body.appendChild(queueHost);
    const queue = await cases.fetchQueue(client, 20, 0);
    cases.renderCaseQueue(queueHost, queue);
    const metricsHost = document.createElement("div");
    document.body.appendChild(metricsHost);
    const metrics = await actions.fetchStaffMetrics(client, "staff queue review");
    actions.renderStaffMetrics(metricsHost, metrics);
  }, { base });
  await expect(page.getByText(/Open cases \(\d+\)/)).toBeVisible();
  await expect(page.getByText(/Effective policy/)).toBeVisible();
  await expect(page.getByText(/no buttons here/)).toBeVisible();
  const buttons = await page.getByRole("button").allTextContents();
  expect(buttons).toHaveLength(0);
  for (const forbidden of ["Ban", "Reverse", "Suspend", "Restore", "Approve"]) {
    expect(page.getByRole("button", { name: forbidden })).toHaveCount(0);
  }

  // Forged calls refuse: unknown staff paths 404, unknown dispositions 400.
  const forged = await postJson(base, "/api/v1/staff/ban", { target: "anyone" }, moderatorCookie);
  expect(forged.status).toBe(404);
  const buyer = await registerAccount(base, {
    displayName: "E2E Staff Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E Staff Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const demandId = await publishDemand(base, buyer.cookie, "E2E Staff Fridge");
  const offered = await postJson(base, `/api/v1/requests/${demandId}/offers`, {
    cycle_number: 1,
    revision_number: 1,
    description: "Staff 300L",
    price: "520.00",
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  }, seller.cookie);
  const offerId = (offered.json as { id: string }).id;
  const reported = await postJson(base, "/api/v1/reports", {
    target_kind: "offer",
    target_id: offerId,
    reason: "spam",
    detail: "staff intake",
  }, buyer.cookie);
  expect(reported.status).toBe(201);
  const caseId = (reported.json as { case_id?: string }).case_id ?? "";
  const badDisposition = await postJson(
    base,
    `/api/v1/moderation/cases/${caseId}/decision`,
    { disposition: "ban", reason: "forged", policy_version: "v1", purpose: "forged attempt" },
    moderatorCookie,
  );
  expect(badDisposition.status).not.toBe(200);
});

test("reasoned review and disposition take inspectable effect", async ({ page, context }) => {
  // Slow-auth environment (documented linear session scan on the
  // disposable stack): more time, same assertions.
  test.slow();
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Case Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E Case Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const demandId = await publishDemand(base, buyer.cookie, "E2E Case Fridge");
  const offered = await postJson(base, `/api/v1/requests/${demandId}/offers`, {
    cycle_number: 1,
    revision_number: 1,
    description: "Case 300L",
    price: "520.00",
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  }, seller.cookie);
  const reported = await postJson(base, "/api/v1/reports", {
    target_kind: "offer",
    target_id: (offered.json as { id: string }).id,
    reason: "spam",
    detail: "case review intake",
  }, buyer.cookie);
  expect(reported.status).toBe(201);
  const caseId = (reported.json as { case_id: string }).case_id;

  const moderatorCookie = await loginAs(base, "+55 11 90000-3104");
  await context.addCookies([{ name: "session", value: sessionToken(moderatorCookie), domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${base}/assets/request-template.html`);
  const outcome = await page.evaluate(async (args: { base: string; kase: string }) => {
    const cases = await import("/assets/pages/staff-cases.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    const before = await cases.fetchQueue(client, 100, 0);
    const seenBefore = before.cases.some((item) => item.case_id === args.kase);
    await cases.openReview(client, args.kase, {
      severity: "normal",
      policyVersion: "v1",
      purpose: "triage for e2e review",
    });
    const host = document.createElement("div");
    document.body.appendChild(host);
    cases.renderCaseQueue(host, await cases.fetchQueue(client, 100, 0));
    await cases.decideCase(client, args.kase, {
      disposition: "valid",
      reason: "fabricated supply pattern",
      policyVersion: "v1",
      purpose: "decide for e2e review",
    });
    const after = await cases.fetchQueue(client, 100, 0);
    return {
      seenBefore,
      seenAfter: after.cases.some((item) => item.case_id === args.kase),
      text: host.textContent ?? "",
    };
  }, { base, kase: caseId });
  expect(outcome.seenBefore).toBe(true);
  expect(outcome.seenAfter).toBe(false);
  expect(outcome.text).not.toMatch(/reporter|phone/i);
});

test("lifting one restriction leaves other prohibitions intact", async ({ page }) => {
  const base = e2eBaseUrl();
  const seed = fixtures();
  expect(seed.suspended_demand).toBeDefined();
  const buyer = await registerAccount(base, {
    displayName: "E2E Scope Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E Scope Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const who = (await (await fetch(`${base}/api/v1/sessions/current`, {
    headers: { cookie: seller.cookie },
  })).json()) as { account_id?: string };
  expect(await (await postJson(base, "/api/v1/blocks", { blocked_user_id: who.account_id ?? "" }, buyer.cookie)).status).toBe(201);

  // Unblock lifts only the relationship restriction: the suspended
  // demand stays unavailable and review of nothing is bypassed.
  const lifted = await fetch(`${base}/api/v1/blocks/${who.account_id ?? ""}`, {
    method: "DELETE",
    headers: { cookie: buyer.cookie, origin: base },
  });
  expect(lifted.status).toBe(200);
  expect(((await lifted.json()) as { removed?: boolean }).removed).toBe(true);
  const suspended = await fetch(`${base}/api/v1/public/requests/${seed.suspended_demand ?? ""}`);
  expect(suspended.status).toBe(404);
  // The same prohibition renders honestly in the UI after the unblock.
  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async (args: { base: string; id: string }) => {
    const pages = await import("/assets/pages/public-request.js");
    const api = await import("/assets/api/client.js");
    const reader = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    const loaded = await pages.loadPublicRequest(reader, args.id);
    const host = document.createElement("div");
    document.body.appendChild(host);
    pages.renderPublicRequest(host, loaded);
  }, { base, id: seed.suspended_demand ?? "" });
  await expect(page.getByText("This request is unavailable.")).toBeVisible();
});
