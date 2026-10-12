/**
 * Minimal safe shell cache: versioned immutable assets only.
 *
 * - Caches same-origin GET shell assets (scripts, styles, manifest,
 *   icon) under a versioned name; activation deletes obsolete shell
 *   caches without touching anything else (no user intent is stored
 *   anywhere here, so nothing of it can be deleted secretly).
 * - Business APIs, contact, owner, staff, and HTML pages stay
 *   network-only: no background mutation replay, no cached
 *   destination, no offline offer or contact fabrication. Offline
 *   failures surface as errors in the calling page.
 */

// Bump together with shell asset changes; old caches drop on activate.
export const SHELL_CACHE = "procurali-shell-v1";

const SHELL_ASSETS = [
  "/assets/main.js",
  "/assets/base.css",
  "/assets/manifest.webmanifest",
  "/assets/icon.svg",
];

interface FetchEventLike {
  request: Request;
  respondWith: (response: Promise<Response> | Response) => void;
  waitUntil: (promise: Promise<unknown>) => void;
}

interface WorkerScope {
  addEventListener: (type: string, listener: (event: FetchEventLike) => void) => void;
  skipWaiting: () => Promise<void>;
  clients: { claim: () => Promise<void> };
}

const scope = self as unknown as WorkerScope;

/** Shell asset URLs eligible for the versioned cache. */
export function isShellAsset(url: string): boolean {
  let path: string;
  try {
    path = new URL(url).pathname;
  } catch {
    return false;
  }
  if (!path.startsWith("/assets/")) {
    return false;
  }
  return (
    path.endsWith(".js") ||
    path.endsWith(".css") ||
    path.endsWith(".webmanifest") ||
    path.endsWith(".svg")
  );
}

/** Business traffic (APIs, HTML pages, mutations) stays network-only. */
export function isNetworkOnly(url: string, method: string): boolean {
  if (method !== "GET") {
    return true;
  }
  let path: string;
  try {
    path = new URL(url).pathname;
  } catch {
    return true;
  }
  return path.includes("/api/") || path.endsWith(".html") || !isShellAsset(url);
}

scope.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(SHELL_CACHE)
      .then((cache) => cache.addAll(SHELL_ASSETS))
      .then(() => scope.skipWaiting()),
  );
});

scope.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((key) => key !== SHELL_CACHE)
            .map((key) => caches.delete(key)),
        ),
      )
      .then(() => scope.clients.claim()),
  );
});

scope.addEventListener("fetch", (event) => {
  const { request } = event;
  if (isNetworkOnly(request.url, request.method)) {
    return;
  }
  event.respondWith(
    caches.match(request).then(
      (hit) =>
        hit ??
        fetch(request).then((response) => {
          const copy = response.clone();
          void caches.open(SHELL_CACHE).then((cache) => cache.put(request, copy));
          return response;
        }),
    ),
  );
});
