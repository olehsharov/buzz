import {
  type PopoutLaunchPayload,
  parsePopoutCommunityRef,
  takePopoutLaunch,
} from "@/features/popout/popoutApi";
import { normalizePopoutRoute } from "@/features/popout/popoutRoute";
import { currentWindowKind } from "@/shared/lib/windowKind";

/**
 * What a pop-out window shows. Window-scoped (not community-scoped): a
 * pop-out is pinned to one community for its whole lifetime, so this is set
 * once before the first render and never reset by community remounts.
 */
export type PopoutSession = {
  /** The community this window belongs to, or null when unknown. */
  communityId: string | null;
  /** Route to show on boot, or null for the "nothing to show" state. */
  initialRoute: string | null;
  /**
   * Opened from a community window: bound natively to that window's
   * community relay, so it runs whatever community the main window is on.
   */
  bound: boolean;
};

// sessionStorage is per window, so it survives a reload of this pop-out
// without leaking into the main window or sibling pop-outs.
const POPOUT_SESSION_STORAGE_KEY = "buzz-popout-session.v1";

let session: PopoutSession | null = null;

/**
 * Pure resolution of a pop-out's session from its sources, in priority order:
 * the one-time native launch payload, then (after a reload) the community
 * remembered in this window's sessionStorage plus the route still in the URL.
 */
export function resolvePopoutSession({
  launch,
  storedCommunityId,
  storedBound = false,
  currentRoute,
}: {
  launch: PopoutLaunchPayload | null;
  storedCommunityId: string | null;
  storedBound?: boolean;
  currentRoute: string | null;
}): PopoutSession {
  const launchCommunity = launch
    ? parsePopoutCommunityRef(launch.community)
    : null;
  const launchRoute = launch ? normalizePopoutRoute(launch.route) : null;
  const bound = launch ? launch.bound === true : storedBound;
  if (launchCommunity && launchRoute) {
    return {
      communityId: launchCommunity.id,
      initialRoute: launchRoute,
      bound,
    };
  }
  return {
    communityId: launchCommunity?.id ?? storedCommunityId,
    initialRoute: normalizePopoutRoute(currentRoute),
    bound,
  };
}

function readStoredSession(): { id: string | null; bound: boolean } {
  try {
    const raw = window.sessionStorage.getItem(POPOUT_SESSION_STORAGE_KEY);
    if (!raw) return { id: null, bound: false };
    const parsed: unknown = JSON.parse(raw);
    return {
      id: parsePopoutCommunityRef(parsed)?.id ?? null,
      bound:
        typeof parsed === "object" &&
        parsed !== null &&
        (parsed as { bound?: unknown }).bound === true,
    };
  } catch {
    return { id: null, bound: false };
  }
}

function storeCommunityId(communityId: string, bound: boolean): void {
  try {
    window.sessionStorage.setItem(
      POPOUT_SESSION_STORAGE_KEY,
      JSON.stringify({ id: communityId, bound }),
    );
  } catch (error) {
    console.warn("Failed to remember the pop-out community:", error);
  }
}

function currentHashRoute(): string | null {
  const hash = window.location.hash;
  return hash.startsWith("#/") ? hash.slice(1) : null;
}

/**
 * Boots the pop-out session before the first render. No-op outside pop-outs.
 * Never throws: an unavailable launch payload degrades to the reload path.
 */
export async function initializePopoutSession(): Promise<PopoutSession | null> {
  if (currentWindowKind() !== "popout") return null;

  let launch: PopoutLaunchPayload | null = null;
  try {
    launch = await takePopoutLaunch();
  } catch (error) {
    console.error("Failed to read the pop-out launch payload:", error);
  }

  const stored = readStoredSession();
  session = resolvePopoutSession({
    launch,
    storedCommunityId: stored.id,
    storedBound: stored.bound,
    currentRoute: currentHashRoute(),
  });
  if (session.communityId) {
    storeCommunityId(session.communityId, session.bound);
  }
  return session;
}

/** The current window's pop-out session; null in main/huddle windows. */
export function getPopoutSession(): PopoutSession | null {
  return session;
}
