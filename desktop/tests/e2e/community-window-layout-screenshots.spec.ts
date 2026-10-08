import { mkdirSync } from "node:fs";

import { expect, type Page, test } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge } from "../helpers/bridge";

/**
 * Community window layout screenshots (main window vs community window).
 * Writes PNGs to `BUZZ_COMMUNITY_WINDOW_SHOTS` (default
 * /tmp/buzz-community-window-shots) with an optional `BUZZ_SHOT_PREFIX`, so
 * a run before and after a layout change can be compared side by side. It
 * also pins the geometry: the community window's sidebar starts at the left
 * edge with the same inset the main window has beside its rail, and the top
 * chrome clears the macOS traffic lights.
 */

const OUT_DIR =
  process.env.BUZZ_COMMUNITY_WINDOW_SHOTS ?? "/tmp/buzz-community-window-shots";
const PREFIX = process.env.BUZZ_SHOT_PREFIX ?? "";

const COMMUNITY_A = {
  id: "shot-a",
  name: "Alpha",
  relayUrl: "ws://localhost:3000",
  addedAt: "2026-01-01T00:00:00.000Z",
};
const COMMUNITY_B = {
  id: "shot-b",
  name: "Bravo",
  relayUrl: "ws://localhost:3001",
  addedAt: "2026-01-02T00:00:00.000Z",
};

async function seed(page: Page) {
  await page.addInitScript(
    ({ list, active }) => {
      window.localStorage.setItem("buzz-communities", JSON.stringify(list));
      window.localStorage.setItem("buzz-active-community-id", active);
    },
    { list: [COMMUNITY_A, COMMUNITY_B], active: COMMUNITY_A.id },
  );
}

async function sidebarBox(page: Page) {
  const box = await page.getByTestId("app-sidebar-scroll-anchor").boundingBox();
  expect(box).not.toBeNull();
  return box as NonNullable<typeof box>;
}

test.beforeAll(() => {
  mkdirSync(OUT_DIR, { recursive: true });
});

test("main window beside the community rail", async ({ page }) => {
  await installMockBridge(page, {}, { skipCommunitySeed: true });
  await seed(page);
  await page.goto("/");
  await expect(page.getByTestId("community-rail")).toBeVisible();
  await waitForAnimations(page);
  await page.screenshot({ path: `${OUT_DIR}/${PREFIX}main-window.png` });
});

test("community window without the rail", async ({ page }) => {
  await installMockBridge(
    page,
    { windowLabel: `community-${COMMUNITY_B.id}` },
    { skipCommunitySeed: true },
  );
  await seed(page);
  await page.goto("/");
  await expect(page.getByTestId("app-sidebar")).toBeVisible();
  await waitForAnimations(page);
  await page.screenshot({ path: `${OUT_DIR}/${PREFIX}community-window.png` });
  await page.screenshot({
    clip: { x: 0, y: 0, width: 360, height: 220 },
    path: `${OUT_DIR}/${PREFIX}community-window-top-left.png`,
  });
});

test("community window lays out like a main window without the rail", async ({
  browser,
}) => {
  // Reference: a main window whose rail is collapsed (one community).
  const single = await browser.newPage();
  await installMockBridge(single, {}, { skipCommunitySeed: true });
  await single.addInitScript((community) => {
    window.localStorage.setItem(
      "buzz-communities",
      JSON.stringify([community]),
    );
    window.localStorage.setItem("buzz-active-community-id", community.id);
  }, COMMUNITY_A);
  await single.goto("/");
  await expect(single.getByTestId("app-sidebar")).toBeVisible();
  await expect(single.getByTestId("community-rail")).toHaveCount(0);
  await waitForAnimations(single);
  await single.screenshot({
    path: `${OUT_DIR}/${PREFIX}main-window-single-community.png`,
  });
  const singleSidebar = await sidebarBox(single);
  const singleSearch = await single
    .getByRole("button", { name: "Search everything" })
    .boundingBox();
  const singleChrome = await single.getByTestId("app-top-chrome").boundingBox();
  const singleToggle = await single
    .getByRole("button", { name: "Toggle Sidebar" })
    .boundingBox();

  const windowPage = await browser.newPage();
  await installMockBridge(
    windowPage,
    { windowLabel: `community-${COMMUNITY_B.id}` },
    { skipCommunitySeed: true },
  );
  await seed(windowPage);
  await windowPage.goto("/");
  await expect(windowPage.getByTestId("app-sidebar")).toBeVisible();
  const windowSidebar = await sidebarBox(windowPage);
  const windowSearch = await windowPage
    .getByRole("button", { name: "Search everything" })
    .boundingBox();
  const windowChrome = await windowPage
    .getByTestId("app-top-chrome")
    .boundingBox();
  const windowToggle = await windowPage
    .getByRole("button", { name: "Toggle Sidebar" })
    .boundingBox();

  // The sidebar never slides under a rail that is not there, and its content
  // sits exactly where a rail-less main window puts it.
  expect(Math.round(windowSidebar.x)).toBe(Math.round(singleSidebar.x));
  expect(Math.round(windowSearch?.x ?? -1)).toBe(
    Math.round(singleSearch?.x ?? -2),
  );
  expect(Math.round(windowSearch?.y ?? -1)).toBe(
    Math.round(singleSearch?.y ?? -2),
  );
  // The top chrome keeps the main window's height and its traffic-light
  // clearance (the toggle sits past the lights), and stays a drag region.
  expect(Math.round(windowChrome?.height ?? -1)).toBe(
    Math.round(singleChrome?.height ?? -2),
  );
  expect(Math.round(windowToggle?.x ?? -1)).toBe(
    Math.round(singleToggle?.x ?? -2),
  );
  await expect(windowPage.getByTestId("app-top-chrome")).toHaveAttribute("data-tauri-drag-region");
  await single.close();
  await windowPage.close();
});
