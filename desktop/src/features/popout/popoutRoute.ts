import {
  defaultParseSearch,
  defaultStringifySearch,
} from "@tanstack/react-router";

/**
 * Destinations that may open in a pop-out window (MVP). Channels and DMs share
 * the channel route; projects, workflows, and settings are deliberately not
 * representable so no surface can offer "open in new window" for them.
 */
export type PopoutDestination =
  | {
      kind: "channel";
      channelId: string;
      /** Scroll to (and highlight) this message on arrival. */
      messageId?: string | null;
      /** Thread root containing `messageId`, when it is a reply. */
      threadRootId?: string | null;
    }
  | { kind: "thread"; channelId: string; threadRootId: string }
  | {
      kind: "forum-post";
      channelId: string;
      postId: string;
      replyId?: string | null;
    }
  | { kind: "profile"; pubkey: string };

export type PopoutDestinationKind = PopoutDestination["kind"];

// Ids are interpolated into an app route, so accept only path/query-safe
// identifier characters. Real ids are UUIDs (channels) or 64-hex (events,
// pubkeys); the looser pattern keeps mock/test ids valid without ever letting
// `/`, `?`, `#`, `&`, or `..` through.
const SAFE_ID = /^[A-Za-z0-9_-]{1,128}$/;

function isSafeId(value: unknown): value is string {
  return typeof value === "string" && SAFE_ID.test(value);
}

function optionalId(value: string | null | undefined): string | undefined {
  return value ? value : undefined;
}

function withSearch(path: string, search: Record<string, string | undefined>) {
  const defined = Object.fromEntries(
    Object.entries(search).filter(([, value]) => value !== undefined),
  );
  return `${path}${defaultStringifySearch(defined)}`;
}

/**
 * Builds the app-relative route (path + query) a pop-out window loads, or
 * `null` when the destination is not an openable MVP destination.
 */
export function buildPopoutRoute(
  destination: PopoutDestination,
): string | null {
  switch (destination.kind) {
    case "channel": {
      const messageId = optionalId(destination.messageId);
      const threadRootId = optionalId(destination.threadRootId);
      if (!isSafeId(destination.channelId)) return null;
      if (messageId !== undefined && !isSafeId(messageId)) return null;
      if (threadRootId !== undefined && !isSafeId(threadRootId)) return null;
      return withSearch(`/channels/${destination.channelId}`, {
        messageId,
        // threadRootId is only meaningful alongside a message target.
        threadRootId: messageId ? threadRootId : undefined,
      });
    }
    case "thread":
      if (
        !isSafeId(destination.channelId) ||
        !isSafeId(destination.threadRootId)
      ) {
        return null;
      }
      return withSearch(`/channels/${destination.channelId}`, {
        thread: destination.threadRootId,
      });
    case "forum-post": {
      const replyId = optionalId(destination.replyId);
      if (!isSafeId(destination.channelId) || !isSafeId(destination.postId)) {
        return null;
      }
      if (replyId !== undefined && !isSafeId(replyId)) return null;
      return withSearch(
        `/channels/${destination.channelId}/posts/${destination.postId}`,
        { replyId },
      );
    }
    case "profile":
      if (!isSafeId(destination.pubkey)) return null;
      return withSearch("/pulse", { profile: destination.pubkey });
    default:
      return null;
  }
}

const CHANNEL_PATH = /^\/channels\/([A-Za-z0-9_-]{1,128})$/;
const FORUM_POST_PATH =
  /^\/channels\/([A-Za-z0-9_-]{1,128})\/posts\/([A-Za-z0-9_-]{1,128})$/;

function readSearch(
  query: string,
  allowedKeys: readonly string[],
): Record<string, string> | null {
  const parsed = defaultParseSearch(query) as Record<string, unknown>;
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(parsed)) {
    if (!allowedKeys.includes(key)) return null;
    if (!isSafeId(value)) return null;
    result[key] = value;
  }
  return result;
}

/**
 * Parses and validates a pop-out route coming back from the native side
 * (launch payload, "open in main window"). Anything that is not exactly an
 * MVP destination route returns `null`, so a malformed or hostile route can
 * never steer a window to an arbitrary screen.
 */
export function parsePopoutRoute(route: unknown): PopoutDestination | null {
  if (typeof route !== "string" || route.length === 0 || route.length > 1024) {
    return null;
  }
  const queryStart = route.indexOf("?");
  const path = queryStart === -1 ? route : route.slice(0, queryStart);
  const query = queryStart === -1 ? "" : route.slice(queryStart);
  if (query.includes("#")) return null;

  const forumMatch = FORUM_POST_PATH.exec(path);
  if (forumMatch) {
    const search = readSearch(query, ["replyId"]);
    if (!search) return null;
    return {
      kind: "forum-post",
      channelId: forumMatch[1],
      postId: forumMatch[2],
      replyId: search.replyId ?? null,
    };
  }

  const channelMatch = CHANNEL_PATH.exec(path);
  if (channelMatch) {
    const search = readSearch(query, ["thread", "messageId", "threadRootId"]);
    if (!search) return null;
    if (search.thread) {
      if (search.messageId || search.threadRootId) return null;
      return {
        kind: "thread",
        channelId: channelMatch[1],
        threadRootId: search.thread,
      };
    }
    if (search.threadRootId && !search.messageId) return null;
    return {
      kind: "channel",
      channelId: channelMatch[1],
      messageId: search.messageId ?? null,
      threadRootId: search.threadRootId ?? null,
    };
  }

  if (path === "/pulse") {
    const search = readSearch(query, ["profile"]);
    if (!search?.profile) return null;
    return { kind: "profile", pubkey: search.profile };
  }

  return null;
}

/** Canonical route for a validated route string, or null when invalid. */
export function normalizePopoutRoute(route: unknown): string | null {
  const destination = parsePopoutRoute(route);
  return destination ? buildPopoutRoute(destination) : null;
}

/**
 * Lenient read of an in-app location (pathname + parsed search) as a pop-out
 * destination: unrelated search keys (panels, highlights) are ignored rather
 * than rejected. Used for a pop-out's own title and "Open in main window".
 */
export function popoutDestinationFromLocation(
  pathname: string,
  search: Record<string, unknown>,
): PopoutDestination | null {
  const pick = (key: string): string | null => {
    const value = search[key];
    return isSafeId(value) ? value : null;
  };
  const forumMatch = FORUM_POST_PATH.exec(pathname);
  if (forumMatch) {
    return {
      kind: "forum-post",
      channelId: forumMatch[1],
      postId: forumMatch[2],
      replyId: pick("replyId"),
    };
  }
  const channelMatch = CHANNEL_PATH.exec(pathname);
  if (channelMatch) {
    const thread = pick("thread");
    if (thread) {
      return {
        kind: "thread",
        channelId: channelMatch[1],
        threadRootId: thread,
      };
    }
    const messageId = pick("messageId");
    return {
      kind: "channel",
      channelId: channelMatch[1],
      messageId,
      threadRootId: messageId ? pick("threadRootId") : null,
    };
  }
  if (pathname === "/pulse") {
    const profile = pick("profile");
    return profile ? { kind: "profile", pubkey: profile } : null;
  }
  return null;
}
