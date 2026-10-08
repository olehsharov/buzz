import { mkdirSync } from "node:fs";

import { expect, type Locator, type Page, test } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge } from "../helpers/bridge";

// Agent defaults are per community. Settings edit the ACTIVE community's
// defaults, say which community that is, and a community switch shows the
// new community's defaults (never the previous one's). The mock bridge keys
// defaults by the applied relay, mirroring the native per-community store.

const SHOTS = "/tmp/buzz-runon-shots";

const COMMUNITY_A = {
  id: "defaults-a",
  name: "Railway",
  relayUrl: "ws://localhost:3000",
  addedAt: "2026-01-01T00:00:00.000Z",
};
const COMMUNITY_B = {
  id: "defaults-b",
  name: "AWS",
  relayUrl: "ws://localhost:3001",
  addedAt: "2026-01-02T00:00:00.000Z",
};
const GATEWAY_KEY = "ANTHROPIC_BASE_URL";

async function seedCommunities(page: Page) {
  await page.addInitScript(
    ({ list, active }) => {
      window.localStorage.setItem("buzz-communities", JSON.stringify(list));
      window.localStorage.setItem("buzz-active-community-id", active);
    },
    { list: [COMMUNITY_A, COMMUNITY_B], active: COMMUNITY_A.id },
  );
}

async function openAgentDefaults(page: Page): Promise<Locator> {
  await page.getByTestId("open-settings").click();
  await page.getByTestId("profile-popover-settings").click();
  await expect(page.getByTestId("settings-view")).toBeVisible();
  await page.getByTestId("settings-nav-agents").click();
  const card = page.getByTestId("settings-global-agent-config");
  await expect(card).toBeVisible({ timeout: 10_000 });
  await expect(card.locator(".animate-spin")).toHaveCount(0, {
    timeout: 5_000,
  });
  // Env vars live under Advanced.
  const advanced = card.getByRole("button", { name: /^Advanced/ });
  if ((await advanced.count()) > 0) {
    await advanced.first().click();
  }
  return card;
}

/** Every value the defaults card shows, text and form fields alike. */
async function shownValues(card: Locator): Promise<string> {
  return card.evaluate((element) => {
    const fields = [...element.querySelectorAll("input, textarea")].map(
      (field) => (field as HTMLInputElement).value,
    );
    return [element.textContent ?? "", ...fields].join("\n");
  });
}

// Tall enough for the whole defaults card, Advanced open.
test.use({ viewport: { width: 1280, height: 1400 } });

test.beforeAll(() => {
  mkdirSync(SHOTS, { recursive: true });
});

test("settings show the active community's agent defaults and a switch changes them", async ({
  page,
}) => {
  await installMockBridge(
    page,
    {
      globalAgentConfigByRelay: {
        [COMMUNITY_A.relayUrl]: {
          env_vars: { [GATEWAY_KEY]: "http://gateway.invalid:4001" },
          provider: "anthropic",
          model: "claude-test",
          preferred_runtime: "buzz-agent",
        },
      },
    },
    { skipCommunitySeed: true },
  );
  await seedCommunities(page);
  await page.goto("/");

  let card = await openAgentDefaults(page);
  await expect(card.getByTestId("agent-defaults-community-name")).toHaveText(
    COMMUNITY_A.name,
  );
  await expect.poll(() => shownValues(card)).toContain(GATEWAY_KEY);
  await waitForAnimations(page);
  await card.screenshot({ path: `${SHOTS}/agent-defaults-railway.png` });

  await page.keyboard.press("Escape");
  await page.getByTestId(`community-rail-button-${COMMUNITY_B.id}`).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.localStorage.getItem("buzz-active-community-id"),
      ),
    )
    .toBe(COMMUNITY_B.id);

  card = await openAgentDefaults(page);
  await expect(card.getByTestId("agent-defaults-community-name")).toHaveText(
    COMMUNITY_B.name,
  );
  const values = await shownValues(card);
  expect(values).not.toContain(GATEWAY_KEY);
  expect(values).not.toContain("gateway.invalid");
  await waitForAnimations(page);
  await card.screenshot({ path: `${SHOTS}/agent-defaults-aws.png` });
});
