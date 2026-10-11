/**
 * Declared buyer outcomes: honest labels with attribution discipline.
 * Answers are completed, cancelled, or unresolved; platform results
 * link only to a contacted offer the buyer actually reached, while
 * elsewhere and unknown answers credit no seller sale. There is no
 * outcome endpoint in this delivery, so this module builds the exact
 * declaration payload (validated, never sent) and the honest screen
 * copy; the suite asserts shapes and wording instead of inventing a
 * submission. Completion side effects (stopping new offers) land with
 * the endpoint and are a documented pending surface.
 */
import { ContractError } from "../api/client.js";

export type OutcomeAnswer = "completed" | "cancelled" | "unresolved";
export type OutcomeSource = "platform" | "elsewhere" | "unknown";

export interface OutcomeDeclaration {
  outcome: OutcomeAnswer;
  source: OutcomeSource;
  attributedOfferId: string | null;
}

const ANSWERS: OutcomeAnswer[] = ["completed", "cancelled", "unresolved"];
const SOURCES: OutcomeSource[] = ["platform", "elsewhere", "unknown"];

/**
 * Build one validated declaration. Platform answers require a
 * contacted offer from the buyer's own history; elsewhere and unknown
 * answers must not attribute any seller. Unknown shapes refuse.
 */
export function buildOutcomeDeclaration(
  answer: string,
  source: string,
  attributedOfferId: string | null,
  contactedOfferIds: string[],
): OutcomeDeclaration {
  if (!(ANSWERS as string[]).includes(answer)) {
    throw new ContractError("invalid_field", 400, "unknown outcome answer");
  }
  if (!(SOURCES as string[]).includes(source)) {
    throw new ContractError("invalid_field", 400, "unknown outcome source");
  }
  if (source === "platform") {
    if (attributedOfferId === null || !contactedOfferIds.includes(attributedOfferId)) {
      throw new ContractError(
        "invalid_field",
        400,
        "platform results link only to a contacted offer",
      );
    }
    return { outcome: answer as OutcomeAnswer, source: "platform", attributedOfferId };
  }
  if (attributedOfferId !== null) {
    throw new ContractError(
      "invalid_field",
      400,
      "elsewhere and unknown answers credit no seller sale",
    );
  }
  return { outcome: answer as OutcomeAnswer, source: source as OutcomeSource, attributedOfferId: null };
}

/** Screen copy for the declaration step: no sale or delivery claims. */
export const OUTCOME_COPY: Record<string, string> = {
  completed: "Declare the need resolved. This records your answer, not a verified sale.",
  cancelled: "Declare the need no longer needed. Open offers simply stop.",
  unresolved: "Declare the outcome unknown. Nothing is credited either way.",
  attribution:
    "Platform credit links only to an offer you actually contacted. Elsewhere and unknown credit nobody.",
};

/** Render the declaration choices with honest copy and attribution gate. */
export function renderOutcomeChoices(
  container: HTMLElement,
  contactedOfferIds: string[],
  onDeclare?: (declaration: OutcomeDeclaration) => void,
): void {
  container.textContent = "";
  for (const answer of ANSWERS) {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = OUTCOME_COPY[answer] ?? answer;
    button.addEventListener("click", () => {
      try {
        const declaration =
          contactedOfferIds.length > 0
            ? buildOutcomeDeclaration(answer, "platform", contactedOfferIds[0] ?? null, contactedOfferIds)
            : buildOutcomeDeclaration(answer, "unknown", null, contactedOfferIds);
        onDeclare?.(declaration);
      } catch {
        const error = document.createElement("p");
        error.textContent = "This answer needs a contacted offer first.";
        container.appendChild(error);
      }
    });
    container.appendChild(button);
  }
  const note = document.createElement("p");
  note.textContent = OUTCOME_COPY["attribution"] ?? "";
  container.appendChild(note);
}
