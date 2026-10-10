/**
 * Route and session behavior (P15-T04): continuation, stale guards,
 * account-switch isolation. Pure logic; no browser or backend needed.
 */
import { test, expect } from "@playwright/test";
import { isOfferActionAllowed, parseRoute, requestPath } from "../src/navigation/router.js";
import { SessionStore } from "../src/state/session.js";
import { createMutationIntent } from "../src/api/client.js";

test("shared request entry returns to the intended resource after login", () => {
  const store = new SessionStore();
  expect(store.isAuthenticated()).toBe(false);
  const route = parseRoute("/r/550e8400-e29b-41d4-a716-446655440000");
  expect(route.name).toBe("public-request");
  store.saveIntentForLogin({
    requestId: route.requestId ?? "",
    draft: { description: "Frost-free 300L", price: "520.00", condition: "used" },
    intentKey: createMutationIntent("rev-1"),
  });
  store.startSession("buyer-1", "Buyer One");
  const intent = store.takePendingIntent();
  expect(intent?.requestId).toBe("550e8400-e29b-41d4-a716-446655440000");
  expect(requestPath(intent?.requestId ?? "")).toBe("/r/550e8400-e29b-41d4-a716-446655440000");
  expect(store.takePendingIntent()).toBeUndefined();
});

test("removed and expired destinations disable the stale offer flow", () => {
  expect(isOfferActionAllowed({ state: "active", visibility: "public" })).toBe(true);
  expect(isOfferActionAllowed({ state: "active", visibility: "visible" })).toBe(true);
  for (const state of ["completed", "cancelled", "expired", "suspended", "removed"]) {
    expect(isOfferActionAllowed({ state, visibility: "public" })).toBe(false);
  }
  expect(isOfferActionAllowed({ state: "active", visibility: "hidden" })).toBe(false);
  expect(isOfferActionAllowed({ state: "active", visibility: "private" })).toBe(false);
  expect(parseRoute("/nope").name).toBe("not-found");
});

test("switching accounts cannot see prior private offers or destination", () => {
  const store = new SessionStore();
  store.startSession("buyer-1", "Buyer One");
  store.cacheOfferView("offer-1", { price: "520.00" });
  store.cacheDestination("contact-1", "hint-for-buyer-1");
  store.saveIntentForLogin({ requestId: "req-1", intentKey: createMutationIntent() });
  store.switchAccount("buyer-2", "Buyer Two");
  expect(store.isAuthenticated()).toBe(true);
  expect(store.currentUser()).toBe("buyer-2");
  expect(store.cachedOfferView("offer-1")).toBeUndefined();
  expect(store.cachedDestination("contact-1")).toBeUndefined();
  expect(store.takePendingIntent()).toBeUndefined();
  store.clearOnLogout();
  expect(store.isAuthenticated()).toBe(false);
  expect(store.currentUser()).toBeUndefined();
});
