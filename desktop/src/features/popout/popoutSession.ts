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
  currentRoute,
}: {
  launch: PopoutLaunchPayload | null;
  storedCommunityId: string | null;
  currentRoute: string | null;
}): PopoutSession {
  const launchCommunity = launch
    ? parsePopoutCommunityRef(launch.community)
    : null;
  const launchRoute = launch ? normalizePopoutRoute(launch.route) : null;
  if (launchCommunity && launchRoute) {
    return { communityId: launchCommunity.id, initialRoute: launchRoute };
  }
  return {
    communityId: launchCommunity?.id ?? storedCommunityId,
    initialRoute: normalizePopoutRoute(currentRoute),
  };
}

function readStoredCommunityId(): string | null {
  try {
    const raw = window.sessionStorage.getItem(POPOUT_SESSION_STORAGE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    return parsePopoutCommunityRef(parsed)?.id ?? null;
  } catch {
    return null;
  }
}

function storeCommunityId(communityId: string): void {
  try {
    window.sessionStorage.setItem(
      POPOUT_SESSION_STORAGE_KEY,
      JSON.stringify({ id: communityId }),
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

  session = resolvePopoutSession({
    launch,
    storedCommunityId: readStoredCommunityId(),
    currentRoute: currentHashRoute(),
  });
  if (session.communityId) storeCommunityId(session.communityId);
  return session;
}

/** The current window's pop-out session; null in main/huddle windows. */
export function getPopoutSession(): PopoutSession | null {
  return session;
}
