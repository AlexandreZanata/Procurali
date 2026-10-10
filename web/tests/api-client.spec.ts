/**
 * API-client behavior (P15-T02): contract mismatch, distinct failures,
 * stable keys, memory-only privacy. Pure logic with mocked fetch; no
 * browser, backend, or network needed.
 */
import { test, expect } from "@playwright/test";
import { ApiClient, createMutationIntent } from "../src/api/client.js";
import { ContractError, parseApiError, parseBudget } from "../src/api/types.js";

function mockFetch(handler: (url: string, init: RequestInit) => Response): typeof fetch {
  return (async (url: unknown, init?: RequestInit) => {
    return handler(String(url), init ?? {});
  }) as typeof fetch;
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

test("typed contract mismatch fails verification", () => {
  expect(parseBudget("520.00")).toBe("520.00");
  expect(() => parseBudget("520.000")).toThrow(ContractError);
  expect(() => parseBudget(520)).toThrow(ContractError);
  expect(() => parseApiError(400, { code: "made_up", message: "?" })).toThrow(ContractError);
  expect(() => parseApiError(200, null)).toThrow(ContractError);
  const known = parseApiError(403, { code: "forbidden_owner", message: "no" });
  expect(known.code).toBe("forbidden_owner");
});

test("denied, stale, validation, and dependency stay distinct", async () => {
  const cases: Array<{ status: number; code: string }> = [
    { status: 403, code: "forbidden_owner" },
    { status: 403, code: "forbidden_role" },
    { status: 409, code: "conflict_revision" },
    { status: 400, code: "invalid_field" },
    { status: 429, code: "rate_limited" },
  ];
  for (const failure of cases) {
    const client = new ApiClient({
      fetchImpl: mockFetch(() => jsonResponse(failure.status, { code: failure.code, message: failure.code })),
    });
    const intent = createMutationIntent("rev-1");
    const error = await client
      .mutate("POST", "/offers", { price: "1.00" }, intent)
      .then(
        () => null,
        (thrown: unknown) => thrown,
      );
    expect(error).toBeInstanceOf(ContractError);
    expect((error as ContractError).code).toBe(failure.code);
    expect((error as ContractError).status).toBe(failure.status);
  }
});

test("retry preserves the key, new mutations differ, logout clears memory", async () => {
  const seen: string[] = [];
  const client = new ApiClient({
    fetchImpl: mockFetch((_url, init) => {
      const headers = new Headers(init.headers as HeadersInit);
      seen.push(headers.get("Idempotency-Key") ?? "");
      return jsonResponse(200, { ok: true });
    }),
  });
  const intent = createMutationIntent();
  await client.mutate("POST", "/offers", { price: "1.00" }, intent);
  await client.mutate("POST", "/offers", { price: "1.00" }, intent);
  expect(seen).toHaveLength(2);
  expect(seen[0]).not.toBe("");
  expect(seen[0]).toBe(seen[1]);
  expect(seen[0]).toBe(intent.key);

  const next = createMutationIntent();
  await client.mutate("POST", "/offers", { price: "2.00" }, next);
  expect(seen[2]).toBe(next.key);
  expect(seen[2]).not.toBe(intent.key);

  client.setCsrfToken("csrf-memory-only");
  expect(client.csrf).toBe("csrf-memory-only");
  client.logout();
  expect(client.csrf).toBeUndefined();
});
