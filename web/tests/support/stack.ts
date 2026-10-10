/**
 * Real-stack harness: disposable backend/database over explicit env only.
 * Every missing input fails loudly (never a skipped success, never a
 * production fallback). Synthetic accounts travel the same public HTTP
 * registration flow as real users; no production-bypass header exists.
 */

export interface AccountFixture {
  displayName: string;
  phone: string;
  city: string;
  region: string;
}

/** Require one non-empty setting without printing its value. */
export function requireEnv(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.trim() === "") {
    throw new Error(`e2e: ${name} is required (no silent skip)`);
  }
  return value;
}

/** Disposable-backend base URL: loopback HTTP only, never production. */
export function e2eBaseUrl(): string {
  const base = requireEnv("PROCURALI_E2E_BASE_URL");
  let parsed: URL;
  try {
    parsed = new URL(base);
  } catch {
    throw new Error("e2e: PROCURALI_E2E_BASE_URL must be an absolute http(s) URL");
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error("e2e: PROCURALI_E2E_BASE_URL must be http(s)");
  }
  if (parsed.hostname !== "127.0.0.1" && parsed.hostname !== "localhost") {
    throw new Error("e2e: refusing non-loopback backend (disposable stacks only)");
  }
  return base.replace(/\/$/, "");
}

/** Disposable-database marker: shape-checked, value never printed. */
export function requireDisposableDb(): void {
  const url = requireEnv("TEST_DATABASE_URL");
  const path = url.split("@").at(-1) ?? "";
  const database = path.split("/").at(-1)?.split("?")[0] ?? "";
  if (!database.startsWith("procurali_test_")) {
    throw new Error("e2e: refusing non-disposable database (must start with procurali_test_)");
  }
}

async function postJson(base: string, path: string, body: unknown): Promise<{ status: number; json: unknown; cookies: string }> {
  const response = await fetch(`${base}${path}`, {
    method: "POST",
    // Same-host origin contract: node fetch sends no Origin by itself,
    // while browsers always do; unsafe methods name their own host.
    headers: { "content-type": "application/json", origin: base },
    body: JSON.stringify(body),
  });
  const text = await response.text();
  const json = text === "" ? null : (JSON.parse(text) as unknown);
  const cookies = response.headers.get("set-cookie") ?? "";
  return { status: response.status, json, cookies };
}

/**
 * Register and activate one synthetic account through the real public
 * flow (accounts, challenges, confirmations). The verification code
 * comes from PROCURALI_E2E_FAKE_CODE, which only the disposable fake
 * provider accepts; a wrong or expired code fails instead of faking
 * success. Returns the account id and its session cookie pair.
 */
export async function registerAccount(
  base: string,
  account: AccountFixture,
): Promise<{ accountId: string; cookie: string }> {
  const code = requireEnv("PROCURALI_E2E_FAKE_CODE");
  const created = await postJson(base, "/api/v1/accounts", {
    display_name: account.displayName,
    phone: account.phone,
    city: account.city,
    region: account.region,
    policy_version: "v1",
  });
  if (created.status !== 201) {
    throw new Error(`e2e: account registration refused (status ${created.status})`);
  }
  const challenged = await postJson(base, "/api/v1/accounts/challenges", { phone: account.phone });
  if (challenged.status !== 202) {
    throw new Error(`e2e: challenge refused (status ${challenged.status})`);
  }
  const confirmed = await postJson(base, "/api/v1/accounts/challenges/confirmations", {
    phone: account.phone,
    code,
  });
  if (confirmed.status !== 200) {
    throw new Error(`e2e: confirmation refused (status ${confirmed.status}); wrong/expired proof never authenticates`);
  }
  const login = await postJson(base, "/api/v1/sessions", { phone: account.phone, code });
  if (login.status !== 200) {
    throw new Error(`e2e: login refused (status ${login.status})`);
  }
  const accountId = (created.json as { account_id?: unknown })?.account_id;
  if (typeof accountId !== "string") {
    throw new Error("e2e: registration receipt carries no account id");
  }
  const cookie = login.cookies.split(";")[0] ?? "";
  if (cookie === "") {
    throw new Error("e2e: login issued no session cookie");
  }
  return { accountId, cookie };
}

/** Assert the disposable backend answers; anything else is failure. */
export async function assertBackendReachable(base: string): Promise<void> {
  let response: Response;
  try {
    response = await fetch(`${base}/api/v1/catalogs/cities`);
  } catch {
    throw new Error("e2e: backend unreachable (no silent skip)");
  }
  if (response.status !== 200) {
    throw new Error(`e2e: backend unhealthy (status ${response.status})`);
  }
}
