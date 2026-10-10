/**
 * Action button: native submit semantics with a pending guard.
 * Labels swap through textContent; innerHTML appears nowhere here.
 */

export interface ActionButtonOptions {
  label: string;
  pendingLabel?: string;
}

/** Build a native submit button with its resting label. */
export function createActionButton(options: ActionButtonOptions): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "submit";
  button.textContent = options.label;
  button.dataset["label"] = options.label;
  if (options.pendingLabel !== undefined) {
    button.dataset["pendingLabel"] = options.pendingLabel;
  }
  return button;
}

/** Toggle the pending guard: disabled plus busy label while pending. */
export function setPending(button: HTMLButtonElement, pending: boolean): void {
  if (pending) {
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
    const pendingLabel = button.dataset["pendingLabel"];
    if (pendingLabel !== undefined) {
      button.textContent = pendingLabel;
    }
  } else {
    button.disabled = false;
    button.removeAttribute("aria-busy");
    const label = button.dataset["label"];
    if (label !== undefined) {
      button.textContent = label;
    }
  }
}

/** Whether the button currently guards against submission. */
export function isPending(button: HTMLButtonElement): boolean {
  return button.disabled;
}
