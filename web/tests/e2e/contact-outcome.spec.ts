/**
 * Contact and outcome e2e (P16-T05) against the real disposable stack:
 * one authorized initiation with a contextual URL, refusals without
 * navigation, and honest declaration shapes. Outcome submission has no
 * endpoint in this delivery, so declarations are validated (never
 * sent) and the suite asserts shapes, wording, and seller-credit
 * discipline instead of inventing the call.
 */
import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { e2eBaseUrl, requireDisposableDb, registerAccount } from "../support/stack.js";

test.describe.configure({ mode: "serial" });

function phone(): string {
  const tail = String(1000 + Math.floor(Math.random() * 9000)).padStart(4, "0");
  return `+55 11 90000-${tail}`;
}

function fixtures(): { retiring_demand?: string; withdrawn_offer?: string } {
  const path = process.env["E2E_FIXTURES_PATH"] ?? "/tmp/procurali-e2e-fixtures.json";
  try {
    return JSON.parse(readFileSync(path, "utf8")) as {
      retiring_demand?: string;
      withdrawn_offer?: string;
    };
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

test("valid contact uses the current seller URL and records one initiation", async ({ page, context }) => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Call Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E Call Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const demandId = await publishDemand(base, buyer.cookie, "E2E Call Fridge");
  const offered = await postJson(base, `/api/v1/requests/${demandId}/offers`, {
    cycle_number: 1,
    revision_number: 1,
    description: "Call 300L",
    price: "520.00",
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  }, seller.cookie);
  expect(offered.status).toBe(201);
  const offerId = (offered.json as { id: string }).id;

  const token = buyer.cookie.split(";")[0]?.split("=").slice(1).join("=") ?? "";
  await context.addCookies([{ name: "session", value: token, domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${base}/assets/request-template.html`);
  const handoff = await page.evaluate(async (args: { base: string; demand: string; offer: string }) => {
    const contact = await import("/assets/pages/contact-action.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    const host = document.createElement("div");
    document.body.appendChild(host);
    const intent = contact.createContactIntent(args.demand, args.offer, 1, "offer_detail");
    const first = await new Promise<unknown>((resolve) => {
      contact.renderContactAction(host, client, intent, "Hello, about your offer", (h) => resolve(h));
      host.querySelector("button")?.click();
    });
    // Replay with the same key: the standing contact answers again.
    const again = await contact.startContact(client, intent, "Hello, about your offer");
    return {
      first: first as { contactId: string; sellerUrl: string; repeat: boolean },
      again: again as { contactId: string; repeat: boolean },
      text: host.textContent ?? "",
    };
  }, { base, demand: demandId, offer: offerId });
  expect(handoff.first.repeat).toBe(false);
  expect(handoff.first.sellerUrl).toMatch(/^https:\/\/wa\.me\/\d+\?text=/);
  expect(handoff.again.contactId).toBe(handoff.first.contactId);
  expect(handoff.again.repeat).toBe(true);
  await expect(page.getByText(/records one initiation/)).toBeVisible();
  expect(handoff.text).not.toMatch(/\d{8,}/);
  expect(page.url().startsWith(base)).toBe(true);
});

test("blocked, withdrawn, and stale offers never navigate to a destination", async ({ page, context }) => {
  const base = e2eBaseUrl();
  const seed = fixtures();
  expect(seed.retiring_demand).toBeDefined();
  expect(seed.withdrawn_offer).toBeDefined();
  const buyer = await registerAccount(base, {
    displayName: "E2E Refuse Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E Refuse Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const demandId = await publishDemand(base, buyer.cookie, "E2E Refuse Fridge");
  const offered = await postJson(base, `/api/v1/requests/${demandId}/offers`, {
    cycle_number: 1,
    revision_number: 1,
    description: "Refuse 300L",
    price: "520.00",
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  }, seller.cookie);
  const offerId = (offered.json as { id: string }).id;

  // Blocked: the buyer blocks the seller, then the handoff refuses.
  const who = await (await fetch(`${base}/api/v1/sessions/current`, {
    headers: { cookie: seller.cookie },
  })).json() as { account_id?: string };
  const blocked = await postJson(base, "/api/v1/blocks", { blocked_user_id: who.account_id ?? "" }, buyer.cookie);
  expect(blocked.status).toBe(201);

  const harnessCookie = await loginAs(base, "+55 11 90000-3101");
  await page.goto(`${base}/assets/request-template.html`);
  const token = buyer.cookie.split(";")[0]?.split("=").slice(1).join("=") ?? "";
  await context.addCookies([{ name: "session", value: token, domain: "127.0.0.1", path: "/" }]);
  const outcome = await page.evaluate(async (args: {
    base: string;
    demand: string;
    offer: string;
  }) => {
    const contact = await import("/assets/pages/contact-action.js");
    const api = await import("/assets/api/client.js");
    const results: string[] = [];
    async function attempt(name: string, demand: string, offer: string, terms: number): Promise<void> {
      const client = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
      try {
        await contact.startContact(
          client,
          contact.createContactIntent(demand, offer, terms, "offer_detail"),
          "Hello",
        );
        results.push(`${name}:sent`);
      } catch (error) {
        results.push(`${name}:${error instanceof api.ContractError ? error.code : "threw"}`);
      }
    }
    // Browser fetch carries the planted buyer cookie automatically.
    await attempt("blocked", args.demand, args.offer, 1);
    await attempt("stale", args.demand, args.offer, 999);
    return results;
  }, { base, demand: demandId, offer: offerId });
  expect(outcome).toHaveLength(2);
  expect(outcome[0]).not.toContain(":sent");
  expect(outcome[1]).not.toContain(":sent");

  // Withdrawn supply refuses over real auth for its own buyer.
  const refused = await postJson(
    base,
    `/api/v1/requests/${seed.retiring_demand ?? ""}/offers/${seed.withdrawn_offer ?? ""}/contact`,
    { handoff_id: "e2e-withdrawn-1", expected_offer_terms: 1, entry_source: "offer_detail" },
    harnessCookie,
  );
  expect(refused.status).not.toBe(200);
  expect(page.url().startsWith(base)).toBe(true);
});

test("declarations stay honest with disciplined attribution", async ({ page }) => {
  const base = e2eBaseUrl();
  await page.goto(`${base}/assets/request-template.html`);
  const outcome = await page.evaluate(async () => {
    const declared = await import("/assets/pages/request-outcome.js");
    const api = await import("/assets/api/client.js");
    const refused: string[] = [];
    for (const [answer, source, attributed, contacted] of [
      ["completed", "platform", null, ["o1"]],
      ["completed", "elsewhere", "o1", ["o1"]],
      ["completed", "unknown", "o1", ["o1"]],
      ["maybe", "unknown", null, []],
      ["completed", "sometimes", null, []],
    ] as const) {
      try {
        declared.buildOutcomeDeclaration(answer, source, attributed, [...contacted]);
        refused.push("accepted");
      } catch (error) {
        refused.push(error instanceof api.ContractError ? error.code : "threw");
      }
    }
    const validPlatform = declared.buildOutcomeDeclaration("completed", "platform", "o1", ["o1", "o2"]);
    const validElsewhere = declared.buildOutcomeDeclaration("cancelled", "elsewhere", null, ["o1"]);
    const validUnknown = declared.buildOutcomeDeclaration("unresolved", "unknown", null, []);
    const host = document.createElement("div");
    document.body.appendChild(host);
    declared.renderOutcomeChoices(host, [], (declaration) => {
      (window as unknown as { __declared: unknown }).__declared = declaration;
    });
    return {
      refused,
      validPlatform,
      validElsewhere,
      validUnknown,
      copy: host.textContent ?? "",
      buttons: host.querySelectorAll("button").length,
    };
  });
  expect(outcome.refused).toEqual([
    "invalid_field",
    "invalid_field",
    "invalid_field",
    "invalid_field",
    "invalid_field",
  ]);
  expect(outcome.validPlatform).toEqual({
    outcome: "completed",
    source: "platform",
    attributedOfferId: "o1",
  });
  expect(outcome.validElsewhere.attributedOfferId).toBeNull();
  expect(outcome.validUnknown.attributedOfferId).toBeNull();
  expect(outcome.buttons).toBe(3);
  expect(outcome.copy).toContain("records your answer, not a verified sale");
  expect(outcome.copy).toContain("credit nobody");
});
