/**
 * Share action: prepared safe message and link with intent-only record.
 * Channels are WhatsApp, copy, or device; the prepared text carries the
 * public item, budget, and region only. A phone-shaped digit run in a
 * prepared message refuses display (never rendered, never shared).
 * Sharing an unpublished demand refuses server-side with nothing
 * recorded. Delivery is never claimed.
 */
import { ApiClient, ContractError, createMutationIntent } from "../api/client.js";
import { renderErrorSummary } from "../components/error-summary.js";

export const SHARE_CHANNELS = ["whatsapp", "copy", "device"] as const;
export type ShareChannel = (typeof SHARE_CHANNELS)[number];

export interface SharePackage {
  link: string;
  message: string;
  channel: string;
}

/** Phone-shaped runs must never appear in a prepared message. */
export function containsPhoneDigits(text: string): boolean {
  const runs = text.match(/\d[\d\s().-]{6,}\d/g) ?? [];
  return runs.some((run) => run.replace(/[^0-9]/g, "").length >= 8);
}

/** Prepare one share intent through the real endpoint. */
export async function prepareShare(
  client: ApiClient,
  requestId: string,
  channel: string,
): Promise<SharePackage> {
  if (!(SHARE_CHANNELS as readonly string[]).includes(channel)) {
    throw new ContractError("invalid_field", 400, "unknown share channel");
  }
  const body = await client.mutate<{
    link?: unknown;
    message?: unknown;
    channel?: unknown;
  }>(
    "POST",
    `/api/v1/public/requests/${requestId}/share`,
    { channel },
    createMutationIntent(),
  );
  if (
    typeof body.link !== "string" ||
    typeof body.message !== "string" ||
    typeof body.channel !== "string"
  ) {
    throw new ContractError("invalid_field", 500, "share receipt malformed");
  }
  if (containsPhoneDigits(body.message)) {
    throw new ContractError("invalid_field", 500, "prepared message carries phone material");
  }
  return { link: body.link, message: body.message, channel: body.channel };
}

/** Render channel choice with the prepared safe message. */
export function renderShareAction(
  container: HTMLElement,
  client: ApiClient,
  requestId: string,
  onPrepared?: (share: SharePackage) => void,
): void {
  container.textContent = "";
  const summary = document.createElement("div");
  container.appendChild(summary);
  const buttons = document.createElement("div");
  container.appendChild(buttons);
  const prepared = document.createElement("div");
  container.appendChild(prepared);
  for (const channel of SHARE_CHANNELS) {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = channel === "copy" ? "Copy link" : `Share via ${channel}`;
    button.addEventListener("click", () => {
      button.disabled = true;
      prepareShare(client, requestId, channel).then(
        (share) => {
          button.disabled = false;
          renderErrorSummary(summary, []);
          prepared.textContent = "";
          const message = document.createElement("p");
          message.textContent = share.message;
          const link = document.createElement("a");
          link.href = share.link;
          link.textContent = share.link;
          prepared.append(message, link);
          onPrepared?.(share);
        },
        (error: unknown) => {
          button.disabled = false;
          renderErrorSummary(summary, [
            error instanceof ContractError ? error.message : "Could not prepare sharing.",
          ]);
        },
      );
    });
    buttons.appendChild(button);
  }
}
