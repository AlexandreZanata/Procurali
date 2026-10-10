/**
 * Memory-only session and safe continuation state.
 *
 * - The session (user id, display name) and the pending shared-request
 *   intent (request id plus unsent offer draft) live in memory only.
 *   This module touches no persistent browser storage and never stores
 *   phones, destinations, tokens, or private offers beyond the current
 *   session's working copies.
 * - No client-side grant exists: `isAuthenticated` only adjusts UI
 *   chrome. A role flag, a route, or a cached copy never authorizes an
 *   action (INV-05); the server decides every mutation.
 * - Login preserves only the permitted intent; logout, revocation, and
 *   account switching clear private views, destinations, and intents.
 */
import type { MutationIntent } from "../api/types.js";

/** Unsent offer draft preserved across login (permitted context only). */
export interface OfferDraft {
  description: string;
  price: string;
  condition: string;
}

/** Shared-request entry preserved across login (AC-32). */
export interface PendingIntent {
  requestId: string;
  draft?: OfferDraft;
  intentKey: MutationIntent;
}

interface PrivateView {
  offers: Map<string, unknown>;
  destination: Map<string, string>;
}

export class SessionStore {
  private userId: string | undefined;
  private displayName: string | undefined;
  private pending: PendingIntent | undefined;
  private views: PrivateView = { offers: new Map(), destination: new Map() };

  /** Whether a session exists (UI chrome only, never authorization). */
  public isAuthenticated(): boolean {
    return this.userId !== undefined;
  }

  public currentUser(): string | undefined {
    return this.userId;
  }

  public startSession(userId: string, displayName: string): void {
    this.userId = userId;
    this.displayName = displayName;
  }

  public currentDisplayName(): string | undefined {
    return this.displayName;
  }

  /** Keep the permitted shared-request context for post-login return. */
  public saveIntentForLogin(intent: PendingIntent): void {
    this.pending = intent;
  }

  /** Consume the preserved intent once; later calls see nothing. */
  public takePendingIntent(): PendingIntent | undefined {
    const intent = this.pending;
    this.pending = undefined;
    return intent;
  }

  /** Cache one private offer view for the current session only. */
  public cacheOfferView(offerId: string, view: unknown): void {
    this.views.offers.set(offerId, view);
  }

  public cachedOfferView(offerId: string): unknown {
    return this.views.offers.get(offerId);
  }

  /** Cache one handoff destination for the current session only. */
  public cacheDestination(contactId: string, hint: string): void {
    this.views.destination.set(contactId, hint);
  }

  public cachedDestination(contactId: string): string | undefined {
    return this.views.destination.get(contactId);
  }

  private clearPrivate(): void {
    this.pending = undefined;
    this.views = { offers: new Map(), destination: new Map() };
  }

  /** Forget the session and every private copy (logout/revocation). */
  public clearOnLogout(): void {
    this.userId = undefined;
    this.displayName = undefined;
    this.clearPrivate();
  }

  /**
   * Move to another account: the prior session's private offers,
   * destinations, and intents never survive the switch.
   */
  public switchAccount(userId: string, displayName: string): void {
    this.clearPrivate();
    this.userId = userId;
    this.displayName = displayName;
  }
}
