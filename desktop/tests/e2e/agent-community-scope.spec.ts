import { expect, type Page, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

// Agents belong to ONE community: each community lists only its own agents,
// and switching communities flips the list. The native `list_managed_agents`
// filters by the active community; the mock bridge mirrors that behind
// `scopeManagedAgentsToCommunity`, so this spec pins the UI half: every
// surface reads the scoped list, and a switch re-reads it.

const COMMUNITY_A = {
  id: "scope-a",
  name: "Alpha",
  relayUrl: "ws://localhost:3000",
  addedAt: "2026-01-01T00:00:00.000Z",
};
const COMMUNITY_B = {
  id: "scope-b",
  name: "Bravo",
  relayUrl: "ws://localhost:3001",
  addedAt: "2026-01-02T00:00:00.000Z",
};

async function seedCommunities(page: Page) {
  await page.addInitScript(
    ({ list, active }) => {
      window.localStorage.setItem("buzz-communities", JSON.stringify(list));
      window.localStorage.setItem("buzz-active-community-id", active);
    },
    { list: [COMMUNITY_A, COMMUNITY_B], active: COMMUNITY_A.id },
  );
}

async function mentionNames(page: Page) {
  await page.getByTestId("channel-general").click();
  const input = page.getByTestId("message-input");
  await input.click();
  await input.fill("@");
  const autocomplete = page.getByTestId("mention-autocomplete");
  await expect(autocomplete).toBeVisible();
  return autocomplete;
}

test("each community lists only its own agents and a switch flips the list", async ({
  page,
}) => {
  await installMockBridge(
    page,
    {
      scopeManagedAgentsToCommunity: true,
      managedAgents: [
        {
          pubkey: "a1".repeat(32),
          name: "AlphaScout",
          status: "running",
          relayUrl: COMMUNITY_A.relayUrl,
        },
        {
          pubkey: "b2".repeat(32),
          name: "BravoScout",
          status: "running",
          relayUrl: COMMUNITY_B.relayUrl,
        },
      ],
    },
    { skipCommunitySeed: true },
  );
  await seedCommunities(page);
  await page.goto("/");

  let autocomplete = await mentionNames(page);
  await expect(autocomplete).toContainText("AlphaScout");
  await expect(autocomplete).not.toContainText("BravoScout");
  await page.getByTestId("message-input").fill("");

  await page.getByTestId(`community-rail-button-${COMMUNITY_B.id}`).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.localStorage.getItem("buzz-active-community-id"),
      ),
    )
    .toBe(COMMUNITY_B.id);

  autocomplete = await mentionNames(page);
  await expect(autocomplete).toContainText("BravoScout");
  await expect(autocomplete).not.toContainText("AlphaScout");

  // The apply that switched communities named the first saved community as
  // the home of legacy (unassigned) agents.
  const applies = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_PAYLOADS__ ?? []).filter(
      (entry) => entry.command === "apply_workspace",
    ),
  );
  expect(applies.length).toBeGreaterThan(0);
  for (const apply of applies) {
    expect(
      (apply.payload as { firstCommunityRelayUrl?: string })
        .firstCommunityRelayUrl,
    ).toBe(COMMUNITY_A.relayUrl);
  }
});
