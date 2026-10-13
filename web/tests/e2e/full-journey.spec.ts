/**
 * Full-journey e2e (P19-T01) against the real disposable stack: the
 * mandatory buyer/seller loop through two contacts, private comparison,
 * and the safety alternatives available over HTTP — with no formal
 * acceptance, payment, or native-app step anywhere (none exists).
 *
 * Outcome declaration has no endpoint in this delivery, so the spec proves
 * the loop completes through contact and that no sale-shaped endpoint can
 * be invented over HTTP (every guess answers 404). All phones below are
 * synthetic and reserved.
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

async function getJson(
  base: string,
  path: string,
  cookie?: string,
): Promise<{ status: number; json: unknown }> {
  const headers: Record<string, string> = {};
  if (cookie !== undefined) headers["cookie"] = cookie;
  const response = await fetch(`${base}${path}`, { headers });
  const text = await response.text();
  return { status: response.status, json: text === "" ? null : (JSON.parse(text) as unknown) };
}

test.beforeAll(() => {
  requireDisposableDb();
  e2eBaseUrl();
  process.env["PROCURALI_E2E_FAKE_CODE"] = "135790";
});

test("mandatory loop: two offers, private comparison, two contacts, no sale step", async () => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Journey Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const sellerOne = await registerAccount(base, {
    displayName: "E2E Journey Seller One",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const sellerTwo = await registerAccount(base, {
    displayName: "E2E Journey Seller Two",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });

  const draft = await postJson(base, "/api/v1/requests/drafts", {
    title: "E2E Journey Fridge",
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  expect(draft.status).toBe(201);
  const demandId = (draft.json as { id: string }).id;
  const published = await postJson(base, `/api/v1/requests/drafts/${demandId}/publication`, {}, buyer.cookie);
  expect([200, 201]).toContain(published.status);

  const offerBody = (price: string) => ({
    cycle_number: 1,
    revision_number: 1,
    description: "Journey 300L",
    price,
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  });
  const first = await postJson(base, `/api/v1/requests/${demandId}/offers`, offerBody("520.00"), sellerOne.cookie);
  expect(first.status).toBe(201);
  const second = await postJson(base, `/api/v1/requests/${demandId}/offers`, offerBody("499.99"), sellerTwo.cookie);
  expect(second.status).toBe(201);
  const offerOne = (first.json as { id: string }).id;
  const offerTwo = (second.json as { id: string }).id;

  // Private comparison: the buyer reads both with no destinations or
  // phones; each seller reads exactly their own offer.
  const compared = await getJson(base, `/api/v1/requests/${demandId}/offers`, buyer.cookie);
  expect(compared.status).toBe(200);
  expect((compared.json as { total: number }).total).toBe(2);
  const rendered = JSON.stringify(compared.json);
  expect(rendered).not.toContain("destination");
  expect(rendered).not.toMatch(/\+55/);
  for (const seller of [sellerOne, sellerTwo]) {
    const mine = await getJson(base, "/api/v1/offers/mine", seller.cookie);
    expect(mine.status).toBe(200);
    expect((mine.json as unknown[]).length).toBe(1);
  }
  const cross = await getJson(base, `/api/v1/requests/${demandId}/offers`, sellerOne.cookie);
  expect(cross.status).toBe(404);

  // Two distinct contacts: the current verified destination renders (the
  // wa.me URL itself is composed client-side and covered by component
  // specs); nothing else phone-shaped travels with it.
  const handoffBody = (offer: string, terms: number) => ({
    handoff_id: crypto.randomUUID(),
    expected_offer_terms: terms,
    entry_source: "offer_detail",
  });
  const contactOne = await postJson(
    base, `/api/v1/requests/${demandId}/offers/${offerOne}/contact`, handoffBody(offerOne, 1), buyer.cookie,
  );
  expect(contactOne.status).toBe(201);
  expect((contactOne.json as { repeat: boolean }).repeat).toBe(false);
  const destinationOne = (contactOne.json as { destination: string }).destination;
  expect(destinationOne).toMatch(/^\+?\d{10,15}$/);
  const contactTwo = await postJson(
    base, `/api/v1/requests/${demandId}/offers/${offerTwo}/contact`, handoffBody(offerTwo, 1), buyer.cookie,
  );
  expect(contactTwo.status).toBe(201);

  // No formal acceptance, payment, or native-app step exists: plausible
  // sale-shaped endpoints all answer 404, so the loop needs none of them.
  for (const guess of [
    `/api/v1/requests/${demandId}/checkout`,
    `/api/v1/requests/${demandId}/accept`,
    `/api/v1/requests/${demandId}/purchase`,
    `/api/v1/requests/${demandId}/sale`,
    `/api/v1/offers/${offerOne}/purchase`,
  ]) {
    const probe = await postJson(base, guess, {}, buyer.cookie);
    expect(probe.status).toBe(404);
  }
});

test("safety alternatives refuse over HTTP without leaking destinations", async () => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Alt Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E Alt Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const draft = await postJson(base, "/api/v1/requests/drafts", {
    title: "E2E Alt Fridge",
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  expect(draft.status).toBe(201);
  const demandId = (draft.json as { id: string }).id;
  const published = await postJson(base, `/api/v1/requests/drafts/${demandId}/publication`, {}, buyer.cookie);
  expect([200, 201]).toContain(published.status);
  const offered = await postJson(base, `/api/v1/requests/${demandId}/offers`, {
    cycle_number: 1,
    revision_number: 1,
    description: "Alt 300L",
    price: "520.00",
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  }, seller.cookie);
  expect(offered.status).toBe(201);
  const offerId = (offered.json as { id: string }).id;

  // Unknown offer: indistinguishable refusal with no destination.
  const ghost = await postJson(base, `/api/v1/requests/${demandId}/offers/${crypto.randomUUID()}/contact`, {
    handoff_id: crypto.randomUUID(),
    expected_offer_terms: 1,
    entry_source: "offer_detail",
  }, buyer.cookie);
  expect(ghost.status).toBe(404);
  expect(JSON.stringify(ghost.json)).not.toContain("destination");

  // Blocked seller: fresh submissions refuse as a relationship refusal.
  const who = await (await fetch(`${base}/api/v1/sessions/current`, {
    headers: { cookie: seller.cookie },
  })).json() as { account_id?: string };
  const blocked = await postJson(base, "/api/v1/blocks", { blocked_user_id: who.account_id ?? "" }, buyer.cookie);
  expect(blocked.status).toBe(201);
  const resubmitted = await postJson(base, `/api/v1/requests/${demandId}/offers`, {
    cycle_number: 1,
    revision_number: 1,
    description: "Alt 300L again",
    price: "510.00",
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  }, seller.cookie);
  expect(resubmitted.status).toBe(403);
  expect((resubmitted.json as { code: string }).code).toBe("forbidden_role");

  // The blocked offer itself refuses contact with no destination.
  const refused = await postJson(base, `/api/v1/requests/${demandId}/offers/${offerId}/contact`, {
    handoff_id: crypto.randomUUID(),
    expected_offer_terms: 1,
    entry_source: "offer_detail",
  }, buyer.cookie);
  expect([403, 404, 409]).toContain(refused.status);
  expect(JSON.stringify(refused.json)).not.toContain("destination");
});
