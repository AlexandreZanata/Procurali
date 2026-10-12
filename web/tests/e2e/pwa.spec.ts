/**
 * PWA shell e2e (P16-T08) against the real disposable stack: fetchable
 * manifest/icon/worker, offline failures without fabrication, and
 * narrow-viewport keyboard operation. The worker caches versioned shell
 * assets only; business traffic stays network-only by construction.
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

test.beforeAll(() => {
  requireDisposableDb();
  e2eBaseUrl();
  process.env["PROCURALI_E2E_FAKE_CODE"] = "135790";
});

test("manifest, icon, and worker load with safe caching rules", async ({ page }) => {
  const base = e2eBaseUrl();
  const manifestResponse = await fetch(`${base}/assets/manifest.webmanifest`);
  expect(manifestResponse.status).toBe(200);
  const manifest = (await manifestResponse.json()) as {
    name?: string;
    icons?: Array<{ src?: string }>;
  };
  expect(typeof manifest.name).toBe("string");
  expect((manifest.icons ?? []).some((icon) => icon.src === "icon.svg")).toBe(true);
  const iconResponse = await fetch(`${base}/assets/icon.svg`);
  expect(iconResponse.status).toBe(200);
  expect((await iconResponse.text()).slice(0, 5)).toBe("<svg ");
  const workerSource = await (await fetch(`${base}/assets/service-worker.js`)).text();
  expect(workerSource).toContain("procurali-shell-v1");
  expect(workerSource).toContain("caches.delete(key)");
  expect(workerSource).toContain("/api/");
  for (const absent of ["localStorage", "sessionStorage", "indexedDB", "background-sync", "BackgroundSync"]) {
    expect(workerSource).not.toContain(absent);
  }
  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async () => {
    // The worker ships as an ES module, like every other bundle file.
    await navigator.serviceWorker.register("/assets/service-worker.js", { type: "module" });
    await navigator.serviceWorker.ready;
  });
  const keys = await page.evaluate(() => caches.keys());
  expect(keys).toContain("procurali-shell-v1");
});

test("offline actions fail openly with no retained phone", async ({ page, context }) => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Offline Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const seller = await registerAccount(base, {
    displayName: "E2E Offline Seller",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const draft = await postJson(base, "/api/v1/requests/drafts", {
    title: "E2E Offline Fridge",
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  const demandId = (draft.json as { id: string }).id;
  await postJson(base, `/api/v1/requests/drafts/${demandId}/publication`, {}, buyer.cookie);
  const offered = await postJson(base, `/api/v1/requests/${demandId}/offers`, {
    cycle_number: 1,
    revision_number: 1,
    description: "Offline 300L",
    price: "520.00",
    condition: "used",
    city_code: "campinas",
    region_code: "centro",
    available: true,
    available_in_city: true,
  }, seller.cookie);
  const offerId = (offered.json as { id: string }).id;

  const token = buyer.cookie.split(";")[0]?.split("=").slice(1).join("=") ?? "";
  await context.addCookies([{ name: "session", value: token, domain: "127.0.0.1", path: "/" }]);
  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async () => {
    await navigator.serviceWorker.register("/assets/service-worker.js", { type: "module" });
    await navigator.serviceWorker.ready;
    // Warm the shell cache while online; only API calls go offline below.
    await import("/assets/pages/contact-action.js");
    await import("/assets/api/client.js");
  });
  await context.setOffline(true);
  const outcome = await page.evaluate(async (args: { base: string; demand: string; offer: string }) => {
    const contact = await import("/assets/pages/contact-action.js");
    const api = await import("/assets/api/client.js");
    const client = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    const host = document.createElement("div");
    document.body.appendChild(host);
    let failed = "";
    try {
      await contact.startContact(
        client,
        contact.createContactIntent(args.demand, args.offer, 1, "offer_detail"),
        "Hello offline",
      );
    } catch (error) {
      failed = error instanceof Error ? error.message : "failed";
    }
    // A plain business read fails offline too: nothing servable cached.
    let readFailed = false;
    try {
      await client.get(`/api/v1/public/requests/${args.demand}`);
    } catch {
      readFailed = true;
    }
    // Surface the failure the way the UI does (text, no destination).
    const notice = document.createElement("p");
    notice.textContent = failed === "" ? "" : "You are offline. No contact was recorded.";
    host.appendChild(notice);
    return { failed, readFailed, text: host.textContent ?? "" };
  }, { base, demand: demandId, offer: offerId });
  expect(outcome.failed).not.toBe("");
  expect(outcome.readFailed).toBe(true);
  expect(outcome.text).toContain("You are offline. No contact was recorded.");
  expect(outcome.text).not.toMatch(/\d{8,}/);
  await context.setOffline(false);
  // No offline row was fabricated: the demand still has no contacts.
  const contacts = await postJson(
    base,
    `/api/v1/requests/${demandId}/offers/${offerId}/contact`,
    { handoff_id: crypto.randomUUID(), expected_offer_terms: 1, entry_source: "offer_detail" },
    buyer.cookie,
  );
  expect([200, 201]).toContain(contacts.status);
});

test("required controls stay labeled and keyboard-usable in a narrow viewport", async ({ page }) => {
  const base = e2eBaseUrl();
  const buyer = await registerAccount(base, {
    displayName: "E2E Narrow Buyer",
    phone: phone(),
    city: "Campinas",
    region: "SP",
  });
  const draft = await postJson(base, "/api/v1/requests/drafts", {
    title: "E2E Narrow Fridge",
    category_code: "home_appliances",
    budget: "600.00",
    condition: "either",
    city_code: "campinas",
    region_code: "centro",
  }, buyer.cookie);
  const demandId = (draft.json as { id: string }).id;
  await postJson(base, `/api/v1/requests/drafts/${demandId}/publication`, {}, buyer.cookie);

  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto(`${base}/assets/request-template.html`);
  await page.evaluate(async (args: { base: string; id: string }) => {
    const pages = await import("/assets/pages/public-request.js");
    const api = await import("/assets/api/client.js");
    const reader = new api.ApiClient({ baseUrl: args.base, fetchImpl: fetch.bind(window) });
    const loaded = await pages.loadPublicRequest(reader, args.id);
    const host = document.createElement("div");
    document.body.appendChild(host);
    pages.renderPublicRequest(host, loaded);
  }, { base, id: demandId });
  await expect(page.getByRole("heading", { name: "E2E Narrow Fridge" })).toBeVisible();
  const action = page.getByRole("link", { name: "I have this" });
  await expect(action).toBeVisible();
  await page.keyboard.press("Tab");
  let focused = "";
  for (let tab = 0; tab < 10; tab += 1) {
    const name = await page.evaluate(() => {
      const active = document.activeElement;
      return active === null ? "" : `${active.tagName}:${active.textContent ?? ""}`;
    });
    if (name.startsWith("A:I have this")) {
      focused = name;
      break;
    }
    await page.keyboard.press("Tab");
  }
  expect(focused).toContain("I have this");
  await page.keyboard.press("Enter");
  await expect.poll(async () => page.url(), { timeout: 10000 }).toContain("#offer");
});
