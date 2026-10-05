import * as React from "react";

import {
  loadActiveCommunityId,
  loadCommunities,
  normalizeRelayUrl,
} from "@/features/communities/communityStorage";
import type { Community } from "@/features/communities/types";
import { getRelayWsUrl } from "@/shared/api/tauri";

/**
 * INTERIM single-relay gate for pop-out windows.
 *
 * The native backend holds ONE applied community (a process-global relay
 * override set by the main window's `apply_workspace`). A pop-out therefore
 * cannot run a different community than the main window without leaking
 * reads/writes into the wrong relay. Until the backend scopes relays per
 * window, a pop-out renders only while its own community is the one the main
 * window has active AND the backend has applied; otherwise it pauses.
 *
 * Everything that decides "may this pop-out run right now?" lives in this
 * module (one predicate + one hook) so per-window communities can replace it
 * without touching any opener site.
 */
export type PopoutCommunityGateStatus =
  /** Waiting for the first backend relay read. */
  | "checking"
  /** Pop-out community == main window's active community == backend relay. */
  | "active"
  /** The main window (or backend) is on another community. */
  | "paused"
  /** The pop-out's community no longer exists on this device. */
  | "missing";

function relayKey(url: string): string {
  return normalizeRelayUrl(url.trim()).replace(/\/+$/, "").toLowerCase();
}

/** Pure gate predicate. `backendRelayUrl === undefined` means "not read yet". */
export function evaluatePopoutCommunityGate({
  popoutCommunityId,
  communities,
  mainActiveCommunityId,
  backendRelayUrl,
}: {
  popoutCommunityId: string;
  communities: readonly Pick<Community, "id" | "relayUrl">[];
  mainActiveCommunityId: string | null;
  backendRelayUrl: string | null | undefined;
}): PopoutCommunityGateStatus {
  const own = communities.find(
    (community) => community.id === popoutCommunityId,
  );
  if (!own) return "missing";
  // Mirror useCommunities: an unknown stored id falls back to the first.
  const mainActive =
    communities.find((community) => community.id === mainActiveCommunityId) ??
    communities[0];
  if (mainActive?.id !== own.id) return "paused";
  if (backendRelayUrl === undefined) return "checking";
  if (backendRelayUrl === null) return "paused";
  return relayKey(backendRelayUrl) === relayKey(own.relayUrl)
    ? "active"
    : "paused";
}

const COMMUNITY_STORAGE_KEYS = new Set([
  "buzz-communities",
  "buzz-active-community-id",
]);
// Storage events are the fast path; the poll is a bounded backstop for
// webviews that do not deliver cross-window storage events and for the
// backend relay, which has no change event.
const GATE_POLL_INTERVAL_MS = 2_000;

export type PopoutCommunityGate = {
  status: PopoutCommunityGateStatus;
  /** The pop-out's own community, when it still exists. */
  community: Community | null;
};

/**
 * Live gate for `popoutCommunityId`. Re-evaluates on cross-window storage
 * changes, focus/visibility, and a bounded poll. Each evaluation is fenced by
 * generation so a slow backend read can never overwrite a newer result.
 */
export function usePopoutCommunityGate(
  popoutCommunityId: string,
): PopoutCommunityGate {
  const [gate, setGate] = React.useState<PopoutCommunityGate>(() => ({
    status: "checking",
    community:
      loadCommunities().find(
        (community) => community.id === popoutCommunityId,
      ) ?? null,
  }));

  React.useEffect(() => {
    let disposed = false;
    let generation = 0;

    const publish = (
      status: PopoutCommunityGateStatus,
      community: Community | null,
    ) =>
      setGate((previous) =>
        previous.status === status &&
        previous.community?.relayUrl === community?.relayUrl &&
        previous.community?.token === community?.token &&
        previous.community?.name === community?.name
          ? previous
          : { status, community },
      );

    const evaluate = () => {
      const current = ++generation;
      const communities = loadCommunities();
      const mainActiveCommunityId = loadActiveCommunityId();
      const community =
        communities.find((entry) => entry.id === popoutCommunityId) ?? null;
      const gateFor = (backendRelayUrl: string | null | undefined) =>
        evaluatePopoutCommunityGate({
          popoutCommunityId,
          communities,
          mainActiveCommunityId,
          backendRelayUrl,
        });
      // Storage alone can already prove a mismatch: pause synchronously,
      // before any backend round-trip, so writes stop as soon as the main
      // window switches away.
      const storageOnly = gateFor(undefined);
      if (storageOnly !== "checking") {
        publish(storageOnly, community);
        return;
      }
      void getRelayWsUrl()
        .then(
          (url) => url,
          (error: unknown) => {
            console.warn("Pop-out could not read the backend relay:", error);
            return null;
          },
        )
        .then((backendRelayUrl) => {
          if (disposed || current !== generation) return;
          publish(gateFor(backendRelayUrl), community);
        });
    };

    const onStorage = (event: StorageEvent) => {
      if (event.key === null || COMMUNITY_STORAGE_KEYS.has(event.key)) {
        evaluate();
      }
    };
    const onVisibility = () => {
      if (document.visibilityState === "visible") evaluate();
    };

    evaluate();
    window.addEventListener("storage", onStorage);
    window.addEventListener("focus", evaluate);
    document.addEventListener("visibilitychange", onVisibility);
    const interval = window.setInterval(evaluate, GATE_POLL_INTERVAL_MS);
    return () => {
      disposed = true;
      window.removeEventListener("storage", onStorage);
      window.removeEventListener("focus", evaluate);
      document.removeEventListener("visibilitychange", onVisibility);
      window.clearInterval(interval);
    };
  }, [popoutCommunityId]);

  return gate;
}
