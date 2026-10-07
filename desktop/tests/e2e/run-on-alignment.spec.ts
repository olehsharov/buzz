/**
 * "Run on" options share one left edge: approved machines (presence dot and a
 * two-line label) line up with "This computer", provider scripts, and the
 * "Add machine…" footer. Also checks the Settings → Machines rows.
 *
 * Screenshots land in /tmp/buzz-runon-shots/ (`RUNON_SHOT_PREFIX` names the
 * set, e.g. "before" / "after").
 */
import { mkdirSync } from "node:fs";

import { expect, type Locator, type Page, test } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge } from "../helpers/bridge";

const SHOTS = "/tmp/buzz-runon-shots";
const PREFIX = process.env.RUNON_SHOT_PREFIX ?? "after";

const HOSTS = [
  {
    pubkey: "a1".repeat(32),
    name: "remote-host",
    os: "linux",
    arch: "aarch64",
    online: false,
  },
  {
    pubkey: "b2".repeat(32),
    name: "studio-mac",
    os: "macos",
    arch: "arm64",
    online: true,
  },
];

const PROVIDERS = [
  { id: "kubernetes", binaryPath: "/usr/local/bin/buzz-backend-kubernetes" },
  { id: "ssh", binaryPath: "/usr/local/bin/buzz-backend-ssh" },
];

/** Left x of the first visible text inside `item` (the label's first glyph). */
async function textStartX(item: Locator): Promise<number> {
  return item.evaluate((element) => {
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, {
      acceptNode: (node) =>
        node.textContent?.trim()
          ? NodeFilter.FILTER_ACCEPT
          : NodeFilter.FILTER_REJECT,
    });
    const node = walker.nextNode();
    if (!node) throw new Error("option has no text");
    const range = document.createRange();
    range.selectNodeContents(node);
    return range.getBoundingClientRect().left;
  });
}

async function openRunOnMenu(page: Page) {
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByTestId("open-agents-view").click();
  await page.getByTestId("new-agent-card").click();
  const dialog = page.getByTestId("persona-dialog");
  await expect(dialog).toBeVisible({ timeout: 10_000 });
  await dialog.getByRole("button", { name: "Advanced", exact: true }).click();
  const trigger = dialog.locator("#agent-run-on");
  await expect(trigger).toBeVisible();
  await trigger.click();
  const menu = page.getByRole("menu");
  await expect(menu).toBeVisible();
  return { trigger, menu };
}

test.beforeAll(() => {
  mkdirSync(SHOTS, { recursive: true });
});

test("every run-on option's text starts at the same x", async ({ page }) => {
  await installMockBridge(page, {
    agentHosts: HOSTS,
    backendProviders: PROVIDERS,
  });
  const { trigger, menu } = await openRunOnMenu(page);

  const radios = menu.getByRole("menuitemradio");
  await expect(radios).toHaveCount(1 + HOSTS.length + PROVIDERS.length);
  const footer = menu.getByRole("menuitem", { name: "Add machine…" });
  await expect(footer).toBeVisible();
  await waitForAnimations(page);

  const triggerBox = await trigger.boundingBox();
  const menuBox = await menu.boundingBox();
  if (triggerBox && menuBox) {
    const top = Math.max(0, Math.min(triggerBox.y, menuBox.y) - 8);
    const bottom =
      Math.max(triggerBox.y + triggerBox.height, menuBox.y + menuBox.height) +
      8;
    await page.screenshot({
      path: `${SHOTS}/${PREFIX}-run-on-dropdown.png`,
      clip: {
        x: triggerBox.x - 8,
        y: top,
        width: triggerBox.width + 16,
        height: bottom - top,
      },
    });
  }

  const labels: string[] = [];
  const starts: number[] = [];
  for (const item of [...(await radios.all()), footer]) {
    labels.push(((await item.textContent()) ?? "").trim());
    starts.push(await textStartX(item));
  }
  const reference = starts[0];
  for (const [index, start] of starts.entries()) {
    expect(
      Math.abs(start - reference),
      `"${labels[index]}" starts at ${start}, "This computer" at ${reference}`,
    ).toBeLessThanOrEqual(0.5);
  }

  // The selected-item indicator still renders in its own slot, left of the
  // shared text edge, for the selected option.
  const selected = menu.locator('[role="menuitemradio"][data-state="checked"]');
  await expect(selected).toHaveCount(1);
  const indicator = selected.locator("svg").first();
  await expect(indicator).toBeVisible();
  const indicatorBox = await indicator.boundingBox();
  expect(indicatorBox).not.toBeNull();
  expect((indicatorBox?.x ?? 0) + (indicatorBox?.width ?? 0)).toBeLessThan(
    reference,
  );

  // The two-line machine label keeps its second line under the first.
  const machine = menu.getByRole("menuitemradio", {
    name: /remote-host/,
  });
  await expect(machine).toContainText("Linux · aarch64 · Offline");
  const lines = machine.locator("span.truncate");
  await expect(lines).toHaveCount(2);
  const [first, second] = [
    await lines.nth(0).boundingBox(),
    await lines.nth(1).boundingBox(),
  ];
  expect(first && second && second.y >= first.y + first.height - 1).toBe(true);
  expect(Math.abs((first?.x ?? 0) - (second?.x ?? 0))).toBeLessThanOrEqual(0.5);
});

test("settings machines rows share one text edge", async ({ page }) => {
  await installMockBridge(page, { agentHosts: HOSTS });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByTestId("open-settings").click();
  await page.getByTestId("profile-popover-settings").click();
  await page.getByTestId("settings-nav-agents").click();
  const machines = page.getByTestId("settings-machines");
  await expect(machines).toBeVisible();
  const rows = machines.locator('li[data-testid^="machine-row-"]');
  await expect(rows).toHaveCount(HOSTS.length);
  await waitForAnimations(page);
  await machines.screenshot({
    path: `${SHOTS}/${PREFIX}-settings-machines.png`,
  });

  const starts: number[] = [];
  for (const row of await rows.all()) {
    starts.push(await textStartX(row));
  }
  for (const start of starts) {
    expect(Math.abs(start - starts[0])).toBeLessThanOrEqual(0.5);
  }
});
