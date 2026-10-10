/**
 * Playwright tiers: component behavior apart from full backend journeys.
 * - `component`: built-bundle specs with no backend (unit-style browser).
 * - `e2e`: real disposable backend/database via PROCURALI_E2E_BASE_URL
 *   (see tests/support/stack.ts); zero e2e cases or a missing backend
 *   fails loudly instead of passing silently.
 */
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  reporter: "line",
  use: {
    baseURL: process.env["PROCURALI_E2E_BASE_URL"],
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "component",
      testMatch: ["*.spec.ts"],
      testIgnore: ["e2e/**", "support/**"],
    },
    {
      name: "e2e",
      testMatch: ["e2e/**/*.spec.ts"],
    },
  ],
});
