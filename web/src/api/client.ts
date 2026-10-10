/**
 * Typed same-origin API client: credentials, CSRF, contract errors, keys.
 *
 * - Same-origin fetch with cookies; unsafe methods carry the in-memory
 *   CSRF token when one was issued (never persisted).
 * - Every mutation takes an explicit idempotency key: retries reuse the
 *   same key, a new intended mutation mints a new one.
 * - Denied, stale, validation, and dependency failures surface as
 *   distinct `ContractError` codes; nothing is collapsed to "error".
 * - Phones, destinations, tokens, and private offers live in memory
 *   only. This module touches no persistent browser storage and logs no
 *   payload: `localStorage` and `sessionStorage` appear nowhere here.
 */
import { ContractError, parseApiError, type MutationIntent } from "./types.js";

export const IDEMPOTENCY_HEADER = "Idempotency-Key";
export const CSRF_HEADER = "X-CSRF-Token";

export interface ClientOptions {
  baseUrl?: string;
  csrfToken?: string;
  fetchImpl?: typeof fetch;
}

function newActionKey(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  const bytes = Array.from({ length: 16 }, () => Math.floor(Math.random() * 256));
  return bytes.map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** Mint one key per intended mutation; retries reuse the same intent. */
export function createMutationIntent(revision?: string): MutationIntent {
  if (revision === undefined) {
    return { key: newActionKey() };
  }
  return { key: newActionKey(), revision };
}

export class ApiClient {
  private readonly baseUrl: string;
  private csrfToken: string | undefined;
  private readonly fetchImpl: typeof fetch;

  public constructor(options?: ClientOptions) {
    this.baseUrl = options?.baseUrl ?? "/api/v1";
    this.csrfToken = options?.csrfToken;
    this.fetchImpl = options?.fetchImpl ?? fetch.bind(globalThis);
  }

  /** Current CSRF token (memory only); absent until issued. */
  public get csrf(): string | undefined {
    return this.csrfToken;
  }

  public setCsrfToken(token: string): void {
    this.csrfToken = token;
  }

  /** Forget private memory: tokens and any caller-held intent key. */
  public logout(): void {
    this.csrfToken = undefined;
  }

  public async get<T>(path: string): Promise<T> {
    return this.request<T>("GET", path);
  }

  public async mutate<T>(
    method: "POST" | "PATCH" | "PUT" | "DELETE",
    path: string,
    body: unknown,
    intent: MutationIntent,
  ): Promise<T> {
    return this.request<T>(method, path, body, intent.key);
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    idempotencyKey?: string,
  ): Promise<T> {
    const headers: Record<string, string> = {
      "content-type": "application/json",
    };
    if (this.csrfToken !== undefined && method !== "GET" && method !== "HEAD") {
      headers[CSRF_HEADER] = this.csrfToken;
    }
    if (idempotencyKey !== undefined) {
      headers[IDEMPOTENCY_HEADER] = idempotencyKey;
    }
    const init: RequestInit = {
      method,
      credentials: "same-origin",
      headers,
    };
    if (body !== undefined) {
      init.body = JSON.stringify(body);
    }
    const response = await this.fetchImpl(`${this.baseUrl}${path}`, init);
    if (response.ok) {
      if (response.status === 204) {
        return undefined as T;
      }
      return (await response.json()) as T;
    }
    let payload: unknown = null;
    try {
      payload = await response.json();
    } catch {
      payload = null;
    }
    throw parseApiError(response.status, payload);
  }
}

export { ContractError };
