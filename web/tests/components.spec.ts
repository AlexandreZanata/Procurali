/**
 * Semantic-primitive behavior (P15-T03) in real Chromium: accessible
 * names, keyboard operation, text-only rendering, pending guard.
 * Loads the built bundle (dist) so the test proves the shipped output.
 * Component modules are dependency-free, so their sources evaluate
 * directly in the page and publish a small test namespace on window.
 */
import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import path from "node:path";

const dist = path.join(process.cwd(), "dist");

function bundle(...files: string[]): string {
  return files
    .map((file) => readFileSync(path.join(dist, "components", file), "utf8"))
    .join("\n")
    .replaceAll("export function", "function")
    .replaceAll("export const", "const");
}

async function loadPrimitives(page: import("@playwright/test").Page): Promise<void> {
  await page.setContent("<main><div data-app></div></main>");
  await page.evaluate(
    bundle("form-field.js", "error-summary.js", "action-button.js") +
      `; window.__primitives = {
        createFormField, setFieldError,
        renderErrorSummary,
        createActionButton, setPending, isPending,
      };`,
  );
}

interface Primitives {
  createFormField: (options: { id: string; label: string }) => HTMLElement;
  renderErrorSummary: (container: HTMLElement, errors: string[]) => number;
  createActionButton: (options: { label: string; pendingLabel?: string }) => HTMLButtonElement;
  setPending: (button: HTMLButtonElement, pending: boolean) => void;
}

test("controls have accessible names and keyboard operation", async ({ page }) => {
  await loadPrimitives(page);
  await page.evaluate(() => {
    const api = (window as unknown as { __primitives: Primitives }).__primitives;
    const app = document.querySelector("[data-app]") as HTMLElement;
    app.appendChild(api.createFormField({ id: "title", label: "Item title" }));
    const button = api.createActionButton({ label: "Publish request" });
    app.appendChild(button);
    let activated = 0;
    button.addEventListener("click", () => {
      activated += 1;
    });
    (window as unknown as { __activatedCount: () => number }).__activatedCount = () => activated;
  });
  await expect(page.getByLabel("Item title")).toBeVisible();
  await expect(page.getByRole("button", { name: "Publish request" })).toBeVisible();
  await page.getByLabel("Item title").click();
  await expect(page.getByLabel("Item title")).toBeFocused();
  await page.getByRole("button", { name: "Publish request" }).focus();
  await page.keyboard.press("Enter");
  const activated = await page.evaluate(
    () => (window as unknown as { __activatedCount: () => number }).__activatedCount(),
  );
  expect(activated).toBe(1);
});

test("user-provided text renders as text and cannot inject markup", async ({ page }) => {
  await loadPrimitives(page);
  const shown = await page.evaluate(() => {
    const api = (window as unknown as { __primitives: Primitives }).__primitives;
    const app = document.querySelector("[data-app]") as HTMLElement;
    const box = document.createElement("div");
    app.appendChild(box);
    const payload = '<img src="x" onerror="alert(1)">';
    const count = api.renderErrorSummary(box, [payload]);
    return {
      count,
      text: box.textContent ?? "",
      images: box.querySelectorAll("img").length,
    };
  });
  expect(shown.count).toBe(1);
  expect(shown.images).toBe(0);
  expect(shown.text).toContain('<img src="x" onerror="alert(1)">');
});

test("disabled pending control neither double-submits nor hides a server error", async ({
  page,
}) => {
  await loadPrimitives(page);
  const outcome = await page.evaluate(() => {
    const api = (window as unknown as { __primitives: Primitives }).__primitives;
    const app = document.querySelector("[data-app]") as HTMLElement;
    const button = api.createActionButton({ label: "Publish request", pendingLabel: "Publishing…" });
    app.appendChild(button);
    const summary = document.createElement("div");
    app.appendChild(summary);
    let submissions = 0;
    button.addEventListener("click", () => {
      submissions += 1;
    });
    api.setPending(button, true);
    const pendingDisabled = button.disabled;
    const pendingLabel = button.textContent;
    button.click();
    const duringPending = submissions;
    api.setPending(button, false);
    const restoredLabel = button.textContent;
    api.renderErrorSummary(summary, ["Offer allowance exhausted"]);
    return {
      pendingDisabled,
      pendingLabel,
      duringPending,
      restoredLabel,
      errorVisible: summary.textContent ?? "",
    };
  });
  expect(outcome.pendingDisabled).toBe(true);
  expect(outcome.pendingLabel).toBe("Publishing…");
  expect(outcome.duringPending).toBe(0);
  expect(outcome.restoredLabel).toBe("Publish request");
  expect(outcome.errorVisible).toContain("Offer allowance exhausted");
});
