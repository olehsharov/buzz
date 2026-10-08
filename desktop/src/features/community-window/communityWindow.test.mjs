import assert from "node:assert/strict";
import test from "node:test";

import { appShellWindowRoles } from "../../app/AppShell.helpers.ts";
import { focusCommunityWindowInsteadOfSwitch } from "./communityWindowGuard.ts";
import { isMainWindowOnlyPath } from "./communityWindowRoutes.ts";

const ID = "0b6c3a9e-2f5f-4f9e-9a43-0b8b2a6f3c11";

test("two-writer guard: an open community window is focused instead of switching", async () => {
  const focused = [];
  const handled = await focusCommunityWindowInsteadOfSwitch(ID, async (id) => {
    focused.push(id);
    return true;
  });
  assert.equal(handled, true);
  assert.deepEqual(focused, [ID]);
});

test("two-writer guard: without a community window the main window switches", async () => {
  assert.equal(
    await focusCommunityWindowInsteadOfSwitch(ID, async () => false),
    false,
  );
  // An unreachable native side never blocks the switch.
  assert.equal(
    await focusCommunityWindowInsteadOfSwitch(ID, async () => {
      throw new Error("ipc down");
    }),
    false,
  );
  // An id that cannot own a window is never asked about.
  let asked = false;
  assert.equal(
    await focusCommunityWindowInsteadOfSwitch("bad/id", async () => {
      asked = true;
      return true;
    }),
    false,
  );
  assert.equal(asked, false);
});

test("gating split: a community window shows chat but owns no app-global work", () => {
  assert.deepEqual(appShellWindowRoles("community", false), {
    isPopout: false,
    isCommunityWindowShell: true,
    // Sidebar, top chrome, channels: shown.
    isAuxWindow: false,
    // Settings, presence, agent sync, reminders, restores: main only.
    isSecondaryWindow: true,
    // Notifications, badge, deep links, tray, rail, mark-all: main only.
    ownsAppGlobals: false,
  });
  assert.deepEqual(appShellWindowRoles("main", false), {
    isPopout: false,
    isCommunityWindowShell: false,
    isAuxWindow: false,
    isSecondaryWindow: false,
    ownsAppGlobals: true,
  });
  const popout = appShellWindowRoles("popout", false);
  assert.equal(popout.isAuxWindow, true);
  assert.equal(popout.isSecondaryWindow, true);
  assert.equal(popout.ownsAppGlobals, false);
  const huddle = appShellWindowRoles("main", true);
  assert.equal(huddle.isAuxWindow, true);
  assert.equal(huddle.isSecondaryWindow, false);
  assert.equal(huddle.ownsAppGlobals, false);
});

test("only app settings are main-window-only in a community window", () => {
  for (const path of ["/settings", "/settings/"]) {
    assert.equal(isMainWindowOnlyPath(path), true, path);
  }
  for (const path of [
    "/",
    "/agents",
    "/projects",
    "/projects/p1",
    "/workflows/w1",
    "/channels/c1",
    "/pulse",
    "/messages/new",
    "/settingsx",
  ]) {
    assert.equal(isMainWindowOnlyPath(path), false, path);
  }
});
