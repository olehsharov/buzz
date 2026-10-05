import { expect, type Locator, type Page, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

/**
 * "Open in new window" (pop-out windows), mock bridge.
 *
 * Main-window specs assert that every input modality on every MVP opener asks
 * the native side for a pop-out (`open_popout_window`) with the destination's
 * route — and that a plain click still navigates in place. Pop-out specs run
 * the app AS a pop-out (window label `popout-<uuid>` + a one-time launch
 * payload) and assert the window shows only its destination, keeps its route
 * over startup restores, never applies the workspace, offers "Open in main
 * window", and pauses while the main window is on another community.
 */

const COMMUNITY_ID = "e2e-default-community";
const GENERAL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const ENGINEERING_ID = "1c7e1c02-87bb-5e88-b2da-5a7a9432d0c9";
const WATERCOOLER_ID = "a27e1ee9-76a6-5bdf-a5d5-1d85610dad11";
const FORUM_POST_ID = "mock-forum-release-thread";
const POPOUT_LABEL = "popout-0f1e2d3c-4b5a-4968-8776-655443322110";

type CommandLogEntry = { command: string; payload: unknown };

async function commandLog(page: Page): Promise<CommandLogEntry[]> {
  return page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMAND_LOG__?: CommandLogEntry[];
        }
      ).__BUZZ_E2E_COMMAND_LOG__ ?? [],
  );
}

async function popoutCalls(page: Page) {
  return (await commandLog(page))
    .filter((entry) => entry.command === "open_popout_window")
    .map((entry) => entry.payload as { route: string; community: unknown });
}

async function clearCommandLog(page: Page) {
  await page.evaluate(() => {
    const log = (
      window as Window & { __BUZZ_E2E_COMMAND_LOG__?: CommandLogEntry[] }
    ).__BUZZ_E2E_COMMAND_LOG__;
    if (log) log.length = 0;
  });
}

async function primaryModifier(page: Page): Promise<"Meta" | "Control"> {
  const isMac = await page.evaluate(() =>
    /mac|iphone|ipad|ipod/i.test(navigator.platform),
  );
  return isMac ? "Meta" : "Control";
}

async function expectPopoutRequest(page: Page, route: string) {
  await expect
    .poll(() => popoutCalls(page))
    .toEqual([{ route, community: { id: COMMUNITY_ID } }]);
}

/**
 * Exercises the three pointer modalities on one opener: platform-modifier
 * click, middle click, and the "Open in new window" context-menu item. Each
 * must request exactly one pop-out for `route` without navigating this window.
 */
async function expectPointerModalitiesOpen(
  page: Page,
  opener: Locator,
  route: string,
) {
  const modifier = await primaryModifier(page);
  const urlBefore = page.url();

  await clearCommandLog(page);
  await opener.click({ modifiers: [modifier] });
  await expectPopoutRequest(page, route);
  expect(page.url()).toBe(urlBefore);

  await clearCommandLog(page);
  await opener.click({ button: "middle" });
  await expectPopoutRequest(page, route);
  expect(page.url()).toBe(urlBefore);

  await clearCommandLog(page);
  await opener.click({ button: "right" });
  const item = page.getByRole("menuitem", { name: "Open in new window" });
  await expect(item).toBeVisible();
  // One accessible stop: the label is the row's only name.
  await expect(item).toHaveAccessibleName("Open in new window");
  await item.click();
  await expectPopoutRequest(page, route);
  expect(page.url()).toBe(urlBefore);
}

test.describe("main window openers", () => {
  test.beforeEach(async ({ page }) => {
    await installMockBridge(page);
  });

  test("sidebar channel rows: every modality pops out; plain click navigates", async ({
    page,
  }) => {
    await page.goto("/");
    const row = page.getByTestId("channel-engineering");
    await expect(row).toBeVisible();

    await expectPointerModalitiesOpen(page, row, `/channels/${ENGINEERING_ID}`);

    // Keyboard: Cmd/Ctrl+Enter on the focused row.
    await clearCommandLog(page);
    await row.focus();
    await page.keyboard.press(`${await primaryModifier(page)}+Enter`);
    await expectPopoutRequest(page, `/channels/${ENGINEERING_ID}`);
    expect(page.url()).not.toContain(ENGINEERING_ID);

    await clearCommandLog(page);
    await row.click();
    await expect(page).toHaveURL(new RegExp(`#/channels/${ENGINEERING_ID}$`));
    await expect(page.getByTestId("chat-title")).toHaveText("engineering");
    expect(await popoutCalls(page)).toEqual([]);
  });

  test("channel link chips in messages pop out", async ({ page }) => {
    await page.goto(`/#/channels/${GENERAL_ID}`);
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
              channelName: "general",
            }) ?? false,
        ),
      )
      .toBe(true);
    await page.evaluate((engineeringId) => {
      window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName: "general",
        content: `see buzz://channel/${engineeringId}`,
      });
    }, ENGINEERING_ID);

    const chip = page.getByRole("button", { name: "Open channel engineering" });
    await expect(chip).toBeVisible();
    // The chip's own right-click menu carries the new-window item.
    const modifier = await primaryModifier(page);
    await clearCommandLog(page);
    await chip.click({ modifiers: [modifier] });
    await expectPopoutRequest(page, `/channels/${ENGINEERING_ID}`);
    await clearCommandLog(page);
    await chip.click({ button: "middle" });
    await expectPopoutRequest(page, `/channels/${ENGINEERING_ID}`);
    await clearCommandLog(page);
    await chip.click({ button: "right" });
    await page
      .locator("[data-buzz-link-context-menu]")
      .getByRole("button", { name: "Open in new window" })
      .click();
    await expectPopoutRequest(page, `/channels/${ENGINEERING_ID}`);
    await clearCommandLog(page);
    await chip.focus();
    await page.keyboard.press(`${modifier}+Enter`);
    await expectPopoutRequest(page, `/channels/${ENGINEERING_ID}`);
    await expect(page.getByTestId("chat-title")).toHaveText("general");
  });

  test("thread summaries ('N replies') pop out the thread", async ({
    page,
  }) => {
    await page.goto(`/#/channels/${GENERAL_ID}`);
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
              channelName: "general",
            }) ?? false,
        ),
      )
      .toBe(true);
    const rootId = await page.evaluate(() => {
      const emit = window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__;
      if (!emit) throw new Error("mock emitter missing");
      const root = emit({ channelName: "general", content: "Pop-out root" });
      emit({
        channelName: "general",
        content: "Pop-out reply",
        parentEventId: root.id,
      });
      return root.id as string;
    });

    const summary = page.locator(
      `[data-testid="message-thread-summary"][data-thread-head-id="${rootId}"]`,
    );
    await expect(summary).toBeVisible();
    await expectPointerModalitiesOpen(
      page,
      summary,
      `/channels/${GENERAL_ID}?thread=${rootId}`,
    );
  });

  test("forum post cards pop out the post", async ({ page }) => {
    await page.goto(`/#/channels/${WATERCOOLER_ID}`);
    const card = page.getByTestId(`forum-post-card-${FORUM_POST_ID}`);
    await expect(card).toBeVisible();
    await expectPointerModalitiesOpen(
      page,
      card,
      `/channels/${WATERCOOLER_ID}/posts/${FORUM_POST_ID}`,
    );
    await clearCommandLog(page);
    await card.focus();
    await page.keyboard.press(`${await primaryModifier(page)}+Enter`);
    await expectPopoutRequest(
      page,
      `/channels/${WATERCOOLER_ID}/posts/${FORUM_POST_ID}`,
    );
  });

  test("profile triggers (message author names) pop out the profile", async ({
    page,
  }) => {
    await page.goto(`/#/channels/${GENERAL_ID}`);
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    // The author name is a UserProfilePopover trigger (role=button).
    const author = page
      .locator('[data-message-id="mock-general-welcome"]')
      .locator('[role="button"]')
      .first();
    await expect(author).toBeVisible();
    await clearCommandLog(page);
    await author.click({ modifiers: [await primaryModifier(page)] });
    await expect.poll(async () => (await popoutCalls(page)).length).toBe(1);
    const [first] = await popoutCalls(page);
    expect(first.route).toMatch(/^\/pulse\?profile=[0-9a-f]{64}$/);
    // Plain click still opens the in-window profile panel, not a window.
    await expectPointerModalitiesOpen(page, author, first.route);
    await clearCommandLog(page);
    await author.focus();
    await page.keyboard.press(`${await primaryModifier(page)}+Enter`);
    await expectPopoutRequest(page, first.route);
  });

  test("search results: Cmd/Ctrl+Enter, modified click, and menu pop out", async ({
    page,
  }) => {
    await page.goto("/");
    await expect(page.getByTestId("app-sidebar")).toBeVisible();
    await page.keyboard.press("ControlOrMeta+k");
    await page.getByTestId("search-dialog-input").fill("engineering");
    const result = page
      .getByTestId("search-results")
      .getByRole("option")
      .filter({ hasText: "engineering" })
      .first();
    await expect(result).toBeVisible();
    await result.hover();

    await clearCommandLog(page);
    await page.keyboard.press(`${await primaryModifier(page)}+Enter`);
    await expectPopoutRequest(page, `/channels/${ENGINEERING_ID}`);
    // Opening dismisses search and leaves this window where it was.
    await expect(page.getByTestId("search-dialog-input")).toBeHidden();
    expect(page.url()).not.toContain(ENGINEERING_ID);

    await page.keyboard.press("ControlOrMeta+k");
    await page.getByTestId("search-dialog-input").fill("engineering");
    await expect(result).toBeVisible();
    await clearCommandLog(page);
    await result.click({ modifiers: [await primaryModifier(page)] });
    await expectPopoutRequest(page, `/channels/${ENGINEERING_ID}`);

    await page.keyboard.press("ControlOrMeta+k");
    await page.getByTestId("search-dialog-input").fill("engineering");
    await expect(result).toBeVisible();
    await clearCommandLog(page);
    await result.click({ button: "right" });
    await page.getByRole("menuitem", { name: "Open in new window" }).click();
    await expectPopoutRequest(page, `/channels/${ENGINEERING_ID}`);

    // Plain Enter still opens in place.
    await page.keyboard.press("ControlOrMeta+k");
    await page.getByTestId("search-dialog-input").fill("engineering");
    await expect(result).toBeVisible();
    await result.hover();
    await clearCommandLog(page);
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(new RegExp(`#/channels/${ENGINEERING_ID}$`));
    expect(await popoutCalls(page)).toEqual([]);
  });

  test("home inbox rows pop out the item's conversation", async ({ page }) => {
    await page.goto("/");
    const row = page.locator('[data-testid^="home-inbox-item-"]').first();
    await expect(row).toBeVisible();
    const itemId = (await row.getAttribute("data-testid"))?.replace(
      "home-inbox-item-",
      "",
    );
    expect(itemId).toBeTruthy();
    const opener = row.getByRole("button", { name: /^Open inbox item from/ });

    const modifier = await primaryModifier(page);
    await clearCommandLog(page);
    await opener.click({ modifiers: [modifier], force: true });
    await expect.poll(async () => (await popoutCalls(page)).length).toBe(1);
    const [call] = await popoutCalls(page);
    expect(call.community).toEqual({ id: COMMUNITY_ID });
    expect(call.route).toMatch(
      new RegExp(`^/channels/[^?]+\\?messageId=${itemId}`),
    );
    expect(page.url()).not.toContain("/channels/");

    await clearCommandLog(page);
    await row.click({ button: "right" });
    await page.getByRole("menuitem", { name: "Open in new window" }).click();
    await expect.poll(async () => (await popoutCalls(page)).length).toBe(1);
  });

  test("the main window follows popout:navigate-main", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByTestId("app-sidebar")).toBeVisible();
    await page.evaluate((route) => {
      void window.__BUZZ_E2E_EMIT_TAURI_EVENT__?.("popout:navigate-main", {
        route,
      });
    }, `/channels/${ENGINEERING_ID}`);
    await expect(page).toHaveURL(new RegExp(`#/channels/${ENGINEERING_ID}$`));
    await expect(page.getByTestId("chat-title")).toHaveText("engineering");

    // Non-destination routes are focus-only: no navigation.
    await page.evaluate(() => {
      void window.__BUZZ_E2E_EMIT_TAURI_EVENT__?.("popout:navigate-main", {
        route: "/settings",
      });
    });
    await page.waitForTimeout(300);
    await expect(page).toHaveURL(new RegExp(`#/channels/${ENGINEERING_ID}$`));
  });
});

async function installPopout(
  page: Page,
  launch: { route: string; community: unknown } | null,
) {
  await installMockBridge(page, {
    windowLabel: POPOUT_LABEL,
    popoutLaunch: launch,
  });
}

test.describe("pop-out window", () => {
  test("shows only its destination, keeps its route, and never applies the workspace", async ({
    page,
  }) => {
    // A remembered "last channel" for this community must not win over the
    // pop-out's launch route.
    await page.addInitScript(
      ({ communityId, generalId }) => {
        window.localStorage.setItem(
          "buzz-community-destinations",
          JSON.stringify({
            [communityId]: { kind: "channel", channelId: generalId },
          }),
        );
      },
      { communityId: COMMUNITY_ID, generalId: GENERAL_ID },
    );
    await installPopout(page, {
      route: `/channels/${ENGINEERING_ID}`,
      community: { id: COMMUNITY_ID },
    });
    await page.goto("/");

    await expect(page.getByTestId("chat-title")).toHaveText("engineering");
    await expect(page).toHaveURL(new RegExp(`#/channels/${ENGINEERING_ID}$`));
    await expect(page.getByTestId("popout-title")).toHaveText("#engineering");
    await expect(page).toHaveTitle("#engineering");
    // No main-window chrome.
    await expect(page.getByTestId("app-sidebar")).toHaveCount(0);
    await expect(page.getByTestId("app-top-chrome")).toHaveCount(0);
    await expect(page.locator('[data-testid^="community-rail"]')).toHaveCount(
      0,
    );

    const commands = (await commandLog(page)).map((entry) => entry.command);
    expect(commands).toContain("take_popout_launch");
    // The backend workspace is the main window's; a pop-out never re-applies
    // it (that would switch the main window's relay).
    expect(commands).not.toContain("apply_workspace");
    expect(commands).not.toContain("set_agent_avatar_communities");
    expect(commands).not.toContain("clear_pending_navigation_deep_links");
    // The pop-out never persists itself as the active community.
    expect(
      await page.evaluate(() =>
        window.localStorage.getItem("buzz-active-community-id"),
      ),
    ).toBe(COMMUNITY_ID);

    // The route survives a reload (the launch payload is one-time).
    await page.reload();
    await expect(page.getByTestId("chat-title")).toHaveText("engineering");
    await expect(page.getByTestId("app-sidebar")).toHaveCount(0);
  });

  test("'Open in main window' hands the route to the main window", async ({
    page,
  }) => {
    await installPopout(page, {
      route: `/channels/${ENGINEERING_ID}`,
      community: { id: COMMUNITY_ID },
    });
    await page.goto("/");
    await expect(page.getByTestId("chat-title")).toHaveText("engineering");

    const button = page.getByRole("button", { name: "Open in main window" });
    await expect(button).toBeVisible();
    await clearCommandLog(page);
    await button.click();
    await expect
      .poll(async () =>
        (await commandLog(page))
          .filter((entry) => entry.command === "focus_main_window_route")
          .map((entry) => entry.payload),
      )
      .toEqual([{ route: `/channels/${ENGINEERING_ID}` }]);
  });

  test("pauses while the main window is on another community, then resumes", async ({
    page,
  }) => {
    await installPopout(page, {
      route: `/channels/${ENGINEERING_ID}`,
      community: { id: COMMUNITY_ID },
    });
    await page.goto("/");
    await expect(page.getByTestId("chat-title")).toHaveText("engineering");

    // The main window adds and switches to another community. Cross-window
    // localStorage changes arrive as `storage` events.
    await page.evaluate((communityId) => {
      const communities = JSON.parse(
        window.localStorage.getItem("buzz-communities") ?? "[]",
      );
      communities.push({
        id: "other-community",
        name: "Other",
        relayUrl: "wss://other.example",
        addedAt: new Date().toISOString(),
      });
      window.localStorage.setItem(
        "buzz-communities",
        JSON.stringify(communities),
      );
      window.localStorage.setItem(
        "buzz-active-community-id",
        "other-community",
      );
      window.dispatchEvent(
        new StorageEvent("storage", {
          key: "buzz-active-community-id",
          oldValue: communityId,
          newValue: "other-community",
        }),
      );
    }, COMMUNITY_ID);

    const paused = page.locator(
      '[data-testid="popout-unavailable"][data-popout-state="paused"]',
    );
    await expect(paused).toBeVisible();
    await expect(paused).toContainText("E2E Test");
    await expect(page.getByTestId("chat-title")).toHaveCount(0);

    // "Show main window" from the paused state only focuses the main
    // window: this window's route belongs to a different community.
    await clearCommandLog(page);
    await paused.getByRole("button", { name: "Show main window" }).click();
    await expect
      .poll(async () =>
        (await commandLog(page))
          .filter((entry) => entry.command === "focus_main_window_route")
          .map((entry) => entry.payload),
      )
      .toEqual([{ route: "/" }]);

    // Main window switches back: the pop-out resumes its destination.
    await page.evaluate((communityId) => {
      window.localStorage.setItem("buzz-active-community-id", communityId);
      window.dispatchEvent(
        new StorageEvent("storage", {
          key: "buzz-active-community-id",
          oldValue: "other-community",
          newValue: communityId,
        }),
      );
    }, COMMUNITY_ID);
    await expect(page.getByTestId("chat-title")).toHaveText("engineering");
    await expect(paused).toHaveCount(0);
  });

  test("ignores main-window-only events (deep links, mouse-nav)", async ({
    page,
  }) => {
    // Tauri's global listen() also delivers events the backend targets at the
    // main window, so the pop-out must not mount those listeners at all.
    await installMockBridge(page, {
      windowLabel: POPOUT_LABEL,
      popoutLaunch: {
        route: `/channels/${ENGINEERING_ID}`,
        community: { id: COMMUNITY_ID },
      },
      pendingNavigationDeepLinks: [
        {
          id: "popout-must-not-consume",
          kind: "channel",
          channelId: GENERAL_ID,
        },
      ],
    });
    await page.goto("/");
    await expect(page.getByTestId("chat-title")).toHaveText("engineering");

    // Build in-window history: engineering -> general via a plain chip click.
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            window.__BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
              channelName: "engineering",
            }) ?? false,
        ),
      )
      .toBe(true);
    await page.evaluate((generalId) => {
      window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName: "engineering",
        content: `over in buzz://channel/${generalId}`,
      });
    }, GENERAL_ID);
    await page.getByRole("button", { name: "Open channel general" }).click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await expect(page.getByTestId("popout-title")).toHaveText("#general");
    const url = page.url();

    await page.evaluate(async (engineeringId) => {
      const emit = window.__BUZZ_E2E_EMIT_TAURI_EVENT__;
      await emit?.("mouse-nav", "back");
      await emit?.("deep-link-channel", { channelId: engineeringId });
      await emit?.("deep-link-message", {
        channelId: engineeringId,
        messageId: "mock-engineering-shipped",
        threadRootId: null,
      });
      await emit?.("popout:navigate-main", {
        route: `/channels/${engineeringId}`,
      });
    }, ENGINEERING_ID);
    await page.waitForTimeout(500);
    await expect.poll(() => page.url()).toBe(url);
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    const commands = (await commandLog(page)).map((entry) => entry.command);
    expect(commands).not.toContain("take_pending_navigation_deep_link");
    expect(commands).not.toContain("take_pending_community_deep_link");
  });

  test("with no launch payload and no route shows a 'nothing to show' state", async ({
    page,
  }) => {
    await installPopout(page, null);
    await page.goto("/");
    const empty = page.locator(
      '[data-testid="popout-unavailable"][data-popout-state="empty"]',
    );
    await expect(empty).toBeVisible();
    await expect(
      empty.getByRole("button", { name: "Show main window" }),
    ).toBeVisible();
    await expect(page.getByTestId("app-sidebar")).toHaveCount(0);
  });
});
