/**
 * Error summary: server errors rendered as text in a native alert region.
 * Every string enters through textContent; innerHTML appears nowhere here.
 */

/** Render server errors into the container; returns the shown count. */
export function renderErrorSummary(container: HTMLElement, errors: string[]): number {
  container.textContent = "";
  if (errors.length === 0) {
    container.removeAttribute("role");
    return 0;
  }
  container.setAttribute("role", "alert");
  const list = document.createElement("ul");
  for (const message of errors) {
    const item = document.createElement("li");
    item.textContent = message;
    list.appendChild(item);
  }
  container.appendChild(list);
  return errors.length;
}
