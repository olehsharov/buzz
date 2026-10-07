import { expect, type Locator, type Page, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

/**
 * Community windows (`community-<id>`), mock bridge.
 *
 * Main-window specs assert that every input modality on a community-rail entry
 * asks the native side to open that community in its own window, never for
 * the active community, and that a plain switch to a community that already
 * has a window focuses it instead (two-writer guard). Community-window specs
 * run the app AS `community-<id>` and assert it binds its own relay instead of
 * applying the workspace, connects its live socket to that relay, shows the
 * chat surface (sidebar, composer) without the rail or agent/project/workflow
 * management, sends in its own community, and lists only that community's
 * agents in mentions.
 */

const COMMUNITY_A = {
  id: "window-a",
  name: "Alpha",
  relayUrl: "ws://localhost:3000",
  addedAt: "2026-01-01T00:00:00.000Z",
};
const COMMUNITY_B = {
  id: "window-b",
  name: "Bravo",
  relayUrl: "ws://localhost:3001",
  addedAt: "2026-01-02T00:00:00.000Z",
};
const AGENTS = [
  {
    pubkey: "a1".repeat(32),
    name: "AlphaScout",
    status: "running" as const,
    relayUrl: COMMUNITY_A.relayUrl,
  },
  {
    pubkey: "b2".repeat(32),
    name: "BravoScout",
    status: "running" as const,
    relayUrl: COMMUNITY_B.relayUrl,
  },
];

type CommandLogEntry = { command: string; payload: unknown };

async function commandLog(page: Page): Promise<CommandLogEntry[]> {
  return page.evaluate(
    () =>
      (
        window as Window & {
          __BUZZ_E2E_COMMAND_PAYLOADS__?: CommandLogEntry[];
        }
      ).__BUZZ_E2E_COMMAND_PAYLOADS__ ?? [],
  );
}

async function payloadsOf(page: Page, command: string) {
  return (await commandLog(page))
    .filter((entry) => entry.command === command)
    .map((entry) => entry.payload);
}

async function clearCommandLog(page: Page) {
  await page.evaluate(() => {
    const target = window as Window & {
      __BUZZ_E2E_COMMAND_PAYLOADS__?: CommandLogEntry[];
    };
    if (target.__BUZZ_E2E_COMMAND_PAYLOADS__) {
      target.__BUZZ_E2E_COMMAND_PAYLOADS__.length = 0;
    }
  });
}

async function primaryModifier(page: Page): Promise<"Meta" | "Control"> {
  const isMac = await page.evaluate(() =>
    /mac|iphone|ipad|ipod/i.test(navigator.platform),
  );
  return isMac ? "Meta" : "Control";
}

async function seedCommunities(page: Page) {
  await page.addInitScript(
    ({ list, active }) => {
      window.localStorage.setItem("buzz-communities", JSON.stringify(list));
      window.localStorage.setItem("buzz-active-community-id", active);
    },
    { list: [COMMUNITY_A, COMMUNITY_B], active: COMMUNITY_A.id },
  );
}

async function activeCommunityId(page: Page) {
  return page.evaluate(() =>
    window.localStorage.getItem("buzz-active-community-id"),
  );
}

async function expectOpenRequest(page: Page) {
  await expect
    .poll(() => payloadsOf(page, "open_community_window"))
    .toEqual([{ communityId: COMMUNITY_B.id, title: COMMUNITY_B.name }]);
  expect(await activeCommunityId(page)).toBe(COMMUNITY_A.id);
}

test.describe("main window: community rail", () => {
  test("Cmd/Ctrl-click, middle click, menu and Cmd/Ctrl+Enter open the community's window", async ({
    page,
  }) => {
    await installMockBridge(page, {}, { skipCommunitySeed: true });
    await seedCommunities(page);
    await page.goto("/");
    const modifier = await primaryModifier(page);
    const railB: Locator = page.getByTestId(
      `community-rail-button-${COMMUNITY_B.id}`,
    );
    await expect(railB).toBeVisible();

    await clearCommandLog(page);
    await railB.click({ modifiers: [modifier] });
    await expectOpenRequest(page);

    await clearCommandLog(page);
    await railB.click({ button: "middle" });
    await expectOpenRequest(page);

    await clearCommandLog(page);
    await railB.click({ button: "right" });
    const item = page.getByRole("menuitem", { name: "Open in new window" });
    await expect(item).toBeVisible();
    await expect(item).toHaveAccessibleName("Open in new window");
    await item.click();
    await expectOpenRequest(page);

    // Keyboard: Cmd/Ctrl+Enter on the focused rail button must open the
    // window and must not start a dnd-kit keyboard drag.
    await clearCommandLog(page);
    await railB.focus();
    await page.keyboard.press(`${modifier}+Enter`);
    await expectOpenRequest(page);
    // dnd-kit marks the dragged item aria-pressed while a drag is active.
    await expect(railB).not.toHaveAttribute("aria-pressed", "true");

    // The active community runs here already: no second window for it.
    await clearCommandLog(page);
    const railA = page.getByTestId(`community-rail-button-${COMMUNITY_A.id}`);
    await railA.click({ modifiers: [modifier] });
    await railA.click({ button: "right" });
    const menuA = page.getByTestId(`community-rail-menu-${COMMUNITY_A.id}`);
    await expect(
      menuA.getByRole("menuitem", { name: "Mark all as read" }),
    ).toBeVisible();
    await expect(
      menuA.getByRole("menuitem", { name: "Open in new window" }),
    ).toHaveCount(0);
    await page.keyboard.press("Escape");
    expect(await payloadsOf(page, "open_community_window")).toEqual([]);
  });

  test("switching to a community that has its own window focuses it instead", async ({
    page,
  }) => {
    await installMockBridge(
      page,
      { openCommunityWindowIds: [COMMUNITY_B.id] },
      { skipCommunitySeed: true },
    );
    await seedCommunities(page);
    await page.goto("/");
    await clearCommandLog(page);
    await page.getByTestId(`community-rail-button-${COMMUNITY_B.id}`).click();
    await expect
      .poll(() => payloadsOf(page, "focus_community_window"))
      .toEqual([{ communityId: COMMUNITY_B.id }]);
    await page.waitForTimeout(300);
    expect(await activeCommunityId(page)).toBe(COMMUNITY_A.id);
    // The main window never applied B's relay.
    expect(
      (await payloadsOf(page, "apply_workspace")).map(
        (payload) => (payload as { relayUrl?: string }).relayUrl,
      ),
    ).not.toContain(COMMUNITY_B.relayUrl);
  });

  test("without a community window a plain click still switches", async ({
    page,
  }) => {
    await installMockBridge(page, {}, { skipCommunitySeed: true });
    await seedCommunities(page);
    await page.goto("/");
    await page.getByTestId(`community-rail-button-${COMMUNITY_B.id}`).click();
    await expect.poll(() => activeCommunityId(page)).toBe(COMMUNITY_B.id);
  });
});

test.describe("community window", () => {
  test.beforeEach(async ({ page }) => {
    await installMockBridge(
      page,
      {
        windowLabel: `community-${COMMUNITY_B.id}`,
        scopeManagedAgentsToCommunity: true,
        managedAgents: AGENTS,
      },
      { skipCommunitySeed: true },
    );
    await seedCommunities(page);
  });

  test("binds its own relay and shows chat without the rail or management", async ({
    page,
  }) => {
    await page.goto("/");
    await expect(page.getByTestId("app-sidebar")).toBeVisible();
    await expect(page.getByTestId("community-window-name")).toHaveText(
      COMMUNITY_B.name,
    );
    await expect(page.locator('[data-testid^="community-rail"]')).toHaveCount(
      0,
    );
    await expect(page.getByTestId("open-agents-view")).toHaveCount(0);
    await expect(page.getByTestId("open-workflows-view")).toHaveCount(0);
    await expect(page.getByTestId("open-projects-view")).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "Show main window" }),
    ).toBeVisible();

    const log = await commandLog(page);
    const commands = log.map((entry) => entry.command);
    expect(await payloadsOf(page, "bind_window_community")).toEqual([
      { relayUrl: COMMUNITY_B.relayUrl },
    ]);
    // The main window owns the workspace; this window never applies it.
    expect(commands).not.toContain("apply_workspace");
    expect(commands).not.toContain("set_agent_avatar_communities");
    // The live relay socket is the window's own community's.
    await expect
      .poll(async () =>
        (await payloadsOf(page, "plugin:websocket|connect")).map(
          (payload) => (payload as { url?: string }).url,
        ),
      )
      .toContain(COMMUNITY_B.relayUrl);
    const connectUrls = (
      await payloadsOf(page, "plugin:websocket|connect")
    ).map((payload) => (payload as { url?: string }).url);
    expect(connectUrls).not.toContain(COMMUNITY_A.relayUrl);
    // Never persists itself as the main window's active community.
    expect(await activeCommunityId(page)).toBe(COMMUNITY_A.id);
  });

  test("sends in its own community and mentions only its own agents", async ({
    page,
  }) => {
    await page.goto("/");
    await page.getByTestId("channel-general").click();
    const input = page.getByTestId("message-input");
    await input.click();
    await input.fill("@");
    const autocomplete = page.getByTestId("mention-autocomplete");
    await expect(autocomplete).toBeVisible();
    await expect(autocomplete).toContainText("BravoScout");
    await expect(autocomplete).not.toContainText("AlphaScout");

    await input.fill("hello from bravo");
    await page.getByTestId("send-message").click();
    await expect(page.getByTestId("message-timeline")).toContainText(
      "hello from bravo",
    );
    // The send rode this window's binding: the workspace was never applied
    // here, so native relay commands resolve this window's relay (pinned per
    // command by the Rust IPC routing tests), and the live socket the send
    // goes out on is this community's.
    const commands = (await commandLog(page)).map((entry) => entry.command);
    expect(commands).not.toContain("apply_workspace");
    const connectUrls = (
      await payloadsOf(page, "plugin:websocket|connect")
    ).map((payload) => (payload as { url?: string }).url);
    expect(new Set(connectUrls)).toEqual(new Set([COMMUNITY_B.relayUrl]));
  });

  test("main-window-only screens point back to the main window", async ({
    page,
  }) => {
    await page.goto("/#/agents");
    await expect(page.getByTestId("community-window-main-only")).toBeVisible();
    await clearCommandLog(page);
    await page
      .getByTestId("community-window-main-only")
      .getByRole("button", { name: "Show main window" })
      .click();
    await expect
      .poll(() => payloadsOf(page, "focus_main_window_route"))
      .toEqual([{ route: "/" }]);
  });
});
