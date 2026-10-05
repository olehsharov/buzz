import assert from "node:assert/strict";
import test from "node:test";

import {
  isNewWindowKeyEvent,
  isNewWindowPointerEvent,
} from "./newWindowGesture.ts";
import { evaluatePopoutCommunityGate } from "./popoutCommunityGate.ts";
import { resolvePopoutSession } from "./popoutSession.ts";
import { popoutWindowTitle } from "./popoutWindowTitle.ts";
import { windowKindFromLabel } from "../../shared/lib/windowKind.ts";

const UUID = "8b7c2d1e-0000-4000-8000-000000000001";
const CHANNEL_ROUTE = `/channels/${UUID}`;

test("windowKindFromLabel classifies main, huddle, and pop-out windows", () => {
  assert.equal(windowKindFromLabel(null), "main");
  assert.equal(windowKindFromLabel("main"), "main");
  assert.equal(windowKindFromLabel(`huddle-${UUID}`), "huddle");
  assert.equal(windowKindFromLabel(`popout-${UUID}`), "popout");
  // Malformed suffixes are not trusted as auxiliary windows.
  assert.equal(windowKindFromLabel("popout-"), "main");
  assert.equal(windowKindFromLabel("popout-not-a-uuid"), "main");
  assert.equal(windowKindFromLabel(`huddle-${UUID}x`), "main");
});

test("isNewWindowPointerEvent: platform modifier click or middle click", () => {
  const click = (overrides) => ({
    button: 0,
    ctrlKey: false,
    metaKey: false,
    ...overrides,
  });
  // macOS
  assert.equal(isNewWindowPointerEvent(click({ metaKey: true }), true), true);
  assert.equal(isNewWindowPointerEvent(click({ ctrlKey: true }), true), false);
  assert.equal(isNewWindowPointerEvent(click({}), true), false);
  // Windows / Linux
  assert.equal(isNewWindowPointerEvent(click({ ctrlKey: true }), false), true);
  assert.equal(isNewWindowPointerEvent(click({ metaKey: true }), false), false);
  // Middle click everywhere; secondary click never.
  assert.equal(isNewWindowPointerEvent(click({ button: 1 }), true), true);
  assert.equal(isNewWindowPointerEvent(click({ button: 1 }), false), true);
  assert.equal(
    isNewWindowPointerEvent(click({ button: 2, metaKey: true }), true),
    false,
  );
});

test("isNewWindowKeyEvent: Cmd/Ctrl+Enter only, never while composing", () => {
  const key = (overrides) => ({
    key: "Enter",
    ctrlKey: false,
    metaKey: false,
    ...overrides,
  });
  assert.equal(isNewWindowKeyEvent(key({ metaKey: true }), true), true);
  assert.equal(isNewWindowKeyEvent(key({ ctrlKey: true }), false), true);
  assert.equal(isNewWindowKeyEvent(key({}), true), false);
  assert.equal(isNewWindowKeyEvent(key({ ctrlKey: true }), true), false);
  assert.equal(
    isNewWindowKeyEvent(key({ key: " ", metaKey: true }), true),
    false,
  );
  assert.equal(
    isNewWindowKeyEvent(
      key({ metaKey: true, nativeEvent: { isComposing: true } }),
      true,
    ),
    false,
  );
});

test("resolvePopoutSession prefers the launch payload", () => {
  assert.deepEqual(
    resolvePopoutSession({
      launch: { route: CHANNEL_ROUTE, community: { id: "c1" } },
      storedCommunityId: "old",
      currentRoute: "/pulse?profile=abc",
    }),
    { communityId: "c1", initialRoute: CHANNEL_ROUTE },
  );
});

test("resolvePopoutSession falls back to the reload state", () => {
  // Reload: the one-time payload is gone; keep the route still in the URL.
  assert.deepEqual(
    resolvePopoutSession({
      launch: null,
      storedCommunityId: "c1",
      currentRoute: CHANNEL_ROUTE,
    }),
    { communityId: "c1", initialRoute: CHANNEL_ROUTE },
  );
  // Nothing to show: no payload and no destination route.
  assert.deepEqual(
    resolvePopoutSession({
      launch: null,
      storedCommunityId: "c1",
      currentRoute: "/",
    }),
    { communityId: "c1", initialRoute: null },
  );
});

test("resolvePopoutSession refuses invalid launch routes and community refs", () => {
  assert.deepEqual(
    resolvePopoutSession({
      launch: { route: "/settings", community: { id: "c1" } },
      storedCommunityId: null,
      currentRoute: null,
    }),
    { communityId: "c1", initialRoute: null },
  );
  assert.deepEqual(
    resolvePopoutSession({
      launch: { route: CHANNEL_ROUTE, community: "c1" },
      storedCommunityId: null,
      currentRoute: null,
    }),
    { communityId: null, initialRoute: null },
  );
});

test("evaluatePopoutCommunityGate requires the main window and backend on this community", () => {
  const communities = [
    { id: "a", relayUrl: "wss://a.example" },
    { id: "b", relayUrl: "wss://b.example" },
  ];
  const gate = (overrides) =>
    evaluatePopoutCommunityGate({
      popoutCommunityId: "a",
      communities,
      mainActiveCommunityId: "a",
      backendRelayUrl: "wss://a.example",
      ...overrides,
    });
  assert.equal(gate({}), "active");
  // Relay URLs compare loosely (trailing slash, host case, missing scheme).
  assert.equal(gate({ backendRelayUrl: "wss://A.example/" }), "active");
  assert.equal(gate({ backendRelayUrl: "a.example" }), "active");
  // The main window switched away: pause before even asking the backend.
  assert.equal(gate({ mainActiveCommunityId: "b" }), "paused");
  assert.equal(
    gate({ mainActiveCommunityId: "b", backendRelayUrl: undefined }),
    "paused",
  );
  // Main switched back but the backend has not applied it yet.
  assert.equal(gate({ backendRelayUrl: "wss://b.example" }), "paused");
  assert.equal(gate({ backendRelayUrl: null }), "paused");
  assert.equal(gate({ backendRelayUrl: undefined }), "checking");
  // Unknown stored id falls back to the first community, like the main window.
  assert.equal(gate({ mainActiveCommunityId: "gone" }), "active");
  assert.equal(gate({ popoutCommunityId: "removed" }), "missing");
});

test("popoutWindowTitle names the channel, DM, thread, or person", () => {
  const channel = { kind: "channel", channelId: UUID };
  assert.equal(
    popoutWindowTitle({ destination: channel, channelLabel: "general" }),
    "#general",
  );
  assert.equal(
    popoutWindowTitle({
      destination: channel,
      channelLabel: "Alice",
      channelIsDm: true,
    }),
    "Alice",
  );
  assert.equal(
    popoutWindowTitle({
      destination: { kind: "thread", channelId: UUID, threadRootId: "r" },
      channelLabel: "general",
    }),
    "Thread in #general",
  );
  assert.equal(
    popoutWindowTitle({
      destination: { kind: "profile", pubkey: "p" },
      profileName: "Bob",
    }),
    "Bob",
  );
  assert.equal(popoutWindowTitle({ destination: null }), "Buzz");
});
