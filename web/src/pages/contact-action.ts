/**
 * Buyer contact action: one gesture, one initiation, honest meaning.
 * A tap posts the eligible handoff with the current revision and a
 * fresh action key; the external seller URL is used only after an
 * authorized response and never as delivery proof. Double taps reuse
 * the same key (the server answers the standing contact with
 * repeat:true) instead of recording a second initiation. Changed terms
 * require a fresh choice before sending. The destination lives in
 * memory only and is never rendered, stored, or logged.
 */
import { ApiClient, ContractError } from "../api/client.js";
import { renderErrorSummary } from "../components/error-summary.js";
import { createActionButton, setPending } from "../components/action-button.js";

export interface ContactIntent {
  requestId: string;
  offerId: string;
  expectedTerms: number;
  entrySource: string;
  key: string;
}

/** Mint one intent per user gesture (key doubles as the handoff id). */
export function createContactIntent(
  requestId: string,
  offerId: string,
  expectedTerms: number,
  entrySource: string,
): ContactIntent {
  const key =
    typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.floor(Math.random() * 1_000_000_000)}`;
  return { requestId, offerId, expectedTerms, entrySource, key };
}

export interface Handoff {
  contactId: string;
  destination: string;
  repeat: boolean;
  sellerUrl: string;
}

function sellerUrl(destination: string, context: string): string {
  const digits = destination.replace(/[^0-9]/g, "");
  return `https://wa.me/${digits}?text=${encodeURIComponent(context)}`;
}

/**
 * Post one eligible handoff. Refusals (blocked, withdrawn, suspended,
 * stale terms) throw without building any URL or retaining anything.
 */
export async function startContact(
  client: ApiClient,
  intent: ContactIntent,
  messageContext: string,
): Promise<Handoff> {
  if (intent.entrySource.trim() === "") {
    throw new ContractError("invalid_field", 400, "entry source is required");
  }
  const body = await client.mutate<{
    contact_id?: unknown;
    destination?: unknown;
    repeat?: unknown;
  }>(
    "POST",
    `/api/v1/requests/${intent.requestId}/offers/${intent.offerId}/contact`,
    {
      handoff_id: intent.key,
      expected_offer_terms: intent.expectedTerms,
      entry_source: intent.entrySource,
    },
    { key: intent.key },
  );
  if (typeof body.contact_id !== "string" || typeof body.destination !== "string") {
    throw new ContractError("invalid_field", 500, "handoff receipt malformed");
  }
  return {
    contactId: body.contact_id,
    destination: body.destination,
    repeat: body.repeat === true,
    sellerUrl: sellerUrl(body.destination, messageContext),
  };
}

/** Meaning disclosure shown beside every contact action. */
export const INITIATION_NOTE =
  "Contact records one initiation with the seller, not a delivered message or a purchase.";

/** Render the contact action with a pending guard and honest meaning. */
export function renderContactAction(
  container: HTMLElement,
  client: ApiClient,
  intent: ContactIntent,
  messageContext: string,
  onHandoff?: (handoff: Handoff) => void,
): void {
  container.textContent = "";
  const summary = document.createElement("div");
  container.appendChild(summary);
  const note = document.createElement("p");
  note.textContent = INITIATION_NOTE;
  container.appendChild(note);
  const action = createActionButton({ label: "Contact seller", pendingLabel: "Contacting…" });
  container.appendChild(action);
  action.addEventListener("click", () => {
    setPending(action, true);
    startContact(client, intent, messageContext).then(
      (handoff) => {
        setPending(action, false);
        renderErrorSummary(summary, []);
        onHandoff?.(handoff);
      },
      (error: unknown) => {
        setPending(action, false);
        renderErrorSummary(summary, [
          error instanceof ContractError
            ? `${error.code}: ${error.message} No contact was recorded.`
            : "Could not contact the seller.",
        ]);
      },
    );
  });
}
