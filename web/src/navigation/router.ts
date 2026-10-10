/**
 * Minimal explicit routes: discovery, request, login, own areas, staff.
 * No framework, no pattern abstraction. Eligibility always stays
 * server-owned: route helpers only disable obviously stale offer flows
 * (terminal or unavailable resources) so the UI never invites an action
 * the server must refuse; every submission is rechecked server-side.
 */

export type RouteName =
  | "discovery"
  | "public-request"
  | "login"
  | "my-requests"
  | "my-offers"
  | "profile"
  | "staff"
  | "not-found";

export interface Route {
  name: RouteName;
  requestId?: string;
}

/** Match one path to an explicit route; anything else is not-found. */
export function parseRoute(path: string): Route {
  const clean = path.split("?")[0]?.split("#")[0] ?? "/";
  if (clean === "/" || clean === "") {
    return { name: "discovery" };
  }
  const request = /^\/r\/([0-9a-fA-F-]{1,64})$/.exec(clean);
  if (request?.[1] !== undefined) {
    return { name: "public-request", requestId: request[1] };
  }
  switch (clean) {
    case "/login":
      return { name: "login" };
    case "/my/requests":
      return { name: "my-requests" };
    case "/my/offers":
      return { name: "my-offers" };
    case "/profile":
      return { name: "profile" };
    case "/staff":
      return { name: "staff" };
    default:
      return { name: "not-found" };
  }
}

/** Resource standing as reported by the server for one request. */
export interface ResourceStanding {
  state: string;
  visibility: string;
}

/**
 * Whether the offer flow may be entered from this client state.
 * False for every terminal or unavailable standing (completed,
 * cancelled, expired, suspended, removed, hidden); true only for a
 * currently active and visible resource. A true answer is not an
 * authorization: the server rechecks eligibility on submission.
 */
export function isOfferActionAllowed(standing: ResourceStanding): boolean {
  if (standing.visibility !== "public" && standing.visibility !== "visible") {
    return false;
  }
  return standing.state === "active";
}

/** Path back to one request for post-login continuation. */
export function requestPath(requestId: string): string {
  return `/r/${requestId}`;
}
