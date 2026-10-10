/** Typed contract shapes mirroring contracts/openapi.yaml (single source). */

export const ERROR_CODES = [
  "unauthenticated",
  "forbidden_owner",
  "forbidden_role",
  "forbidden_origin",
  "forbidden_state",
  "invalid_field",
  "missing_field",
  "text_too_long",
  "unknown_city",
  "not_found",
  "conflict_revision",
  "idempotency_key_reuse",
  "rate_limited",
] as const;

export type ErrorCode = (typeof ERROR_CODES)[number];

export interface ApiErrorBody {
  code: string;
  message: string;
  fields?: Record<string, string>;
}

export class ContractError extends Error {
  public readonly code: ErrorCode;
  public readonly status: number;
  public readonly fields: Record<string, string>;

  public constructor(code: ErrorCode, status: number, message: string, fields?: Record<string, string>) {
    super(message);
    this.name = "ContractError";
    this.code = code;
    this.status = status;
    this.fields = fields ?? {};
  }
}

function isErrorCode(code: string): code is ErrorCode {
  return (ERROR_CODES as readonly string[]).includes(code);
}

/** Parse one contract error body: unknown codes fail instead of guessing. */
export function parseApiError(status: number, body: unknown): ContractError {
  if (typeof body !== "object" || body === null) {
    throw new ContractError("invalid_field", status, "malformed error body");
  }
  const record = body as Record<string, unknown>;
  if (typeof record["code"] !== "string" || !isErrorCode(record["code"])) {
    throw new ContractError("invalid_field", status, "unknown contract error code");
  }
  const message = typeof record["message"] === "string" ? record["message"] : record["code"] as string;
  const fields: Record<string, string> = {};
  if (typeof record["fields"] === "object" && record["fields"] !== null) {
    for (const [key, value] of Object.entries(record["fields"] as Record<string, unknown>)) {
      if (typeof value === "string") {
        fields[key] = value;
      }
    }
  }
  return new ContractError(record["code"], status, message, fields);
}

const BUDGET_PATTERN = /^[0-9]+(\.[0-9]{1,2})?$/;

/** Parse exact decimal money: overprecision fails instead of rounding. */
export function parseBudget(raw: unknown): string {
  if (typeof raw !== "string" || !BUDGET_PATTERN.test(raw)) {
    throw new ContractError("invalid_field", 400, "budget must be an exact decimal string");
  }
  return raw;
}

export interface Page<T> {
  items: T[];
  nextCursor: string | null;
}

export interface PublicRequest {
  id: string;
  title: string;
  budget: string;
  condition: string;
  category: string;
  city: string;
  regionLabel: string;
  publishedAt: string;
  deadline: string;
  cycle: number;
  revision: string;
}

export interface Ratio {
  numerator: number;
  denominator: number;
  value: number | null;
}

export interface OperationalAggregate {
  incidents: {
    rawReports: number;
    validIncidents: number;
    eligibleInteractions: number;
    validIncidentRate: Ratio;
    rawReportRate: Ratio;
    medianReviewDelayHours: number | null;
  };
  sharing: {
    shareIntents: number;
    totalLandings: number;
    identifiableLandings: number;
    directVisits: number;
  };
  professionals: {
    declared: number;
    freeActive: number;
  };
  policyVersion: string;
}

/** One intended mutation: its idempotency key plus the revision composed against. */
export interface MutationIntent {
  key: string;
  revision?: string;
}
