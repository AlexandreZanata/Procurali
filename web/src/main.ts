/** Minimal native entry: no runtime framework, no network calls. */

export const APP_NAME = "procurali";

export function hello(name: string): string {
  const trimmed = name.trim();
  if (trimmed.length === 0) {
    return `${APP_NAME}: ready`;
  }
  return `${APP_NAME}: ready for ${trimmed}`;
}

function bootstrap(): void {
  if (typeof document === "undefined") {
    return;
  }
  const root = document.querySelector("[data-app]");
  if (root !== null) {
    root.textContent = hello("");
  }
}

bootstrap();
