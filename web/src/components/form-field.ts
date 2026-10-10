/**
 * Labeled form field: native label/input association, field errors.
 * Text enters through textContent only; innerHTML appears nowhere here.
 */

export interface FormFieldOptions {
  id: string;
  label: string;
  type?: string;
  value?: string;
  error?: string;
}

/** Build one labeled field: label, native input, and error slot. */
export function createFormField(options: FormFieldOptions): HTMLElement {
  const wrapper = document.createElement("div");
  wrapper.className = "field";

  const label = document.createElement("label");
  label.htmlFor = options.id;
  label.textContent = options.label;
  wrapper.appendChild(label);

  const input = document.createElement("input");
  input.id = options.id;
  input.name = options.id;
  input.type = options.type ?? "text";
  if (options.value !== undefined) {
    input.value = options.value;
  }
  if (options.error !== undefined) {
    input.setAttribute("aria-invalid", "true");
  }
  wrapper.appendChild(input);

  const error = document.createElement("p");
  error.className = "error";
  error.setAttribute("role", "alert");
  error.textContent = options.error ?? "";
  wrapper.appendChild(error);

  return wrapper;
}

/** Replace the field error text (empty clears) and reflect validity. */
export function setFieldError(wrapper: HTMLElement, message: string): void {
  const error = wrapper.querySelector("p.error");
  if (error !== null) {
    error.textContent = message;
  }
  const input = wrapper.querySelector("input");
  if (input !== null) {
    if (message === "") {
      input.removeAttribute("aria-invalid");
    } else {
      input.setAttribute("aria-invalid", "true");
    }
  }
}
