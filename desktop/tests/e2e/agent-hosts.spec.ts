/**
 * Agent hosts ("machines") in the desktop UI, against the mock bridge's
 * hosts test double (src/testing/e2eBridgeHosts.ts).
 *
 * Covers:
 *  - "Where to run" lists This computer + only the approved machines, with
 *    presence; an offline machine is listed, described with last-seen, and
 *    cannot be picked; selection works from the keyboard
 *  - "Add machine…" (dropdown footer, keyboard) shows one install-and-pair
 *    command served by the community relay, the code's expiry and "New
 *    code", then the code; Approve confirms the code, and the
 *    machine's details are shown once it is added (grant sent)
 *  - Settings → Machines lists machines with agents on them; Forget machine
 *    confirms, calls forget, and removes the row
 */
import { mkdirSync } from "node:fs";

import { expect, test } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge } from "../helpers/bridge";

type Page = import("@playwright/test").Page;

const SHOTS = "/tmp/buzz-hosts-shots";
const ALPHA = "a1".repeat(32);
const BRAVO = "b2".repeat(32);
const NEW_HOST = "c3".repeat(32);

const HOSTS = [
  {
    pubkey: ALPHA,
    name: "alpha-box",
    os: "linux",
    arch: "x86_64",
    online: true,
  },
  {
    pubkey: BRAVO,
    name: "bravo-mac",
    os: "macos",
    arch: "arm64",
    online: false,
    lastSeenSecsAgo: 300,
  },
];

async function commandPayloads(page: Page, command: string) {
  return page.evaluate(
    (name) =>
      (
        (
          window as Window & {
            __BUZZ_E2E_COMMAND_PAYLOADS__?: {
              command: string;
              payload: unknown;
            }[];
          }
        ).__BUZZ_E2E_COMMAND_PAYLOADS__ ?? []
      )
        .filter((entry) => entry.command === name)
        .map((entry) => entry.payload),
    command,
  );
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
  await trigger.focus();
  await trigger.press("Enter");
  const menu = page.getByRole("menu");
  await expect(menu).toBeVisible();
  return { dialog, trigger, menu };
}

test.beforeAll(() => {
  mkdirSync(SHOTS, { recursive: true });
});

test("where to run lists only approved machines with presence; offline is disabled", async ({
  page,
}) => {
  await installMockBridge(page, { agentHosts: HOSTS });
  const { dialog, trigger, menu } = await openRunOnMenu(page);

  const options = menu.getByRole("menuitemradio");
  await expect(options).toHaveCount(3);
  await expect(options.nth(0)).toHaveText("This computer");
  const alpha = menu.getByRole("menuitemradio", { name: /alpha-box/ });
  const bravo = menu.getByRole("menuitemradio", { name: /bravo-mac/ });
  await expect(alpha).toContainText("Linux · x86_64 · Online");
  await expect(alpha).not.toHaveAttribute("aria-disabled", "true");
  await expect(bravo).toContainText(
    "macOS · arm64 · Offline, last seen 5 min ago",
  );
  await expect(bravo).toHaveAttribute("aria-disabled", "true");
  await expect(
    menu.getByRole("menuitem", { name: "Add machine…" }),
  ).toBeVisible();
  await waitForAnimations(page);
  // Trigger + open menu (the menu portals outside the dialog box).
  const triggerBox = await trigger.boundingBox();
  const menuBox = await menu.boundingBox();
  if (triggerBox && menuBox) {
    // The menu may open above or below the trigger: clip their union.
    const top = Math.min(triggerBox.y - 32, menuBox.y) - 8;
    const bottom =
      Math.max(triggerBox.y + triggerBox.height, menuBox.y + menuBox.height) +
      8;
    await page.screenshot({
      path: `${SHOTS}/01-where-to-run-dropdown.png`,
      clip: {
        x: triggerBox.x - 8,
        y: Math.max(0, top),
        width: triggerBox.width + 16,
        height: bottom - Math.max(0, top),
      },
    });
  }

  // Keyboard: Radix skips the disabled machine; Enter on alpha selects it.
  await alpha.focus();
  await alpha.press("Enter");
  await expect(trigger).toHaveAttribute("aria-expanded", "false");
  await expect(trigger).toContainText("alpha-box");
  await expect(dialog.getByTestId("where-to-run-host-note")).toContainText(
    "Runs on alpha-box",
  );

  // Clicking the offline machine does not select it.
  await trigger.press("Enter");
  await bravo.click({ force: true });
  await expect(trigger).toContainText("alpha-box");
});

test("add machine: install command, code, approve sends the grant", async ({
  page,
}) => {
  await installMockBridge(page, { agentHosts: HOSTS });
  const { menu } = await openRunOnMenu(page);

  // Reach the footer command from the keyboard.
  const addMachine = menu.getByRole("menuitem", { name: "Add machine…" });
  await addMachine.focus();
  await addMachine.press("Enter");

  const dialog = page.getByTestId("add-machine-dialog");
  await expect(dialog).toBeVisible();
  // One command: install from the community relay's /host, then pair with
  // this session's URI. Never `--relay` (it would override the pairing relay).
  const install = dialog.getByTestId("add-machine-install-command");
  await expect(install).toHaveText(
    /^curl -fsSL 'https?:\/\/[^/']+\/host\/install\.sh' \| bash -s -- --base 'https?:\/\/[^/']+\/host' --uri 'nostrpair:\/\/[^']+'$/,
  );
  await expect(install).not.toContainText("--relay");
  await expect(install).not.toContainText("example.invalid");
  await expect(dialog.getByTestId("add-machine-up-command")).toHaveText(
    /^buzz host up --uri 'nostrpair:\/\/[^']+'$/,
  );
  // The commands carry the URI of the session this dialog started.
  const [pairingUri] = (await commandPayloads(page, "get_host_install_info"))
    .map((payload) => (payload as { pairingUri?: string }).pairingUri)
    .filter(Boolean);
  expect(pairingUri).toMatch(/^nostrpair:\/\//);
  await expect(install).toContainText(`--uri '${pairingUri}'`);
  await expect(dialog.getByTestId("add-machine-expiry")).toHaveText(
    /^Code expires in 2:(0\d|10)\.$/,
  );
  expect(await commandPayloads(page, "start_host_pairing")).toHaveLength(1);

  // "New code" starts a fresh session.
  await dialog.getByTestId("add-machine-new-code").click();
  await expect
    .poll(
      async () => (await commandPayloads(page, "start_host_pairing")).length,
    )
    .toBe(2);
  await expect(install).toBeVisible();

  // The machine scans the code: the six-digit code appears.
  await page.evaluate(
    ([hostPubkey]) =>
      window.__BUZZ_E2E_HOST_PAIRING_OFFER__?.("482913", {
        host_pubkey: hostPubkey,
        name: "charlie-vm",
        os: "linux",
        arch: "aarch64",
      }),
    [NEW_HOST],
  );
  await expect(dialog.getByTestId("add-machine-sas")).toHaveText("482 913");
  await expect(dialog.getByTestId("add-machine-status")).toContainText(
    "Code 482 913 shown",
  );
  await waitForAnimations(page);
  await dialog.screenshot({ path: `${SHOTS}/02-add-machine-code.png` });

  // Keyboard approve.
  await dialog.getByTestId("add-machine-approve").focus();
  await page.keyboard.press("Enter");

  const done = dialog.getByTestId("add-machine-done");
  await expect(done).toContainText("charlie-vm · Linux · aarch64");
  expect(await commandPayloads(page, "confirm_pairing_sas")).toHaveLength(1);
  const grants = await page.evaluate(
    () => window.__BUZZ_E2E_HOST_GRANTS__ ?? [],
  );
  expect(grants).toEqual([expect.objectContaining({ host_pubkey: NEW_HOST })]);
  await waitForAnimations(page);
  await dialog.screenshot({ path: `${SHOTS}/03-add-machine-done.png` });

  await dialog.getByTestId("add-machine-close").click();
  await expect(dialog).toHaveCount(0);
  // The new machine is now in Where to run.
  const trigger = page.getByTestId("persona-dialog").locator("#agent-run-on");
  await trigger.press("Enter");
  await expect(
    page.getByRole("menu").getByRole("menuitemradio", { name: /charlie-vm/ }),
  ).toBeVisible();
});

test("settings machines list and forget machine", async ({ page }) => {
  await installMockBridge(page, {
    agentHosts: HOSTS,
    managedAgents: [
      {
        pubkey: "d4".repeat(32),
        name: "builder",
        status: "deployed",
        backend: { type: "host", host_pubkey: ALPHA },
      },
    ],
  });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByTestId("open-settings").click();
  await page.getByTestId("profile-popover-settings").click();
  await page.getByTestId("settings-nav-agents").click();

  const machines = page.getByTestId("settings-machines");
  await expect(machines).toBeVisible({ timeout: 10_000 });
  const alphaRow = machines.getByTestId(`machine-row-${ALPHA}`);
  await expect(alphaRow).toContainText("alpha-box");
  await expect(alphaRow).toContainText("Linux · x86_64 · Online");
  await expect(alphaRow).toContainText("Agents: builder");
  await expect(machines.getByTestId(`machine-row-${BRAVO}`)).toContainText(
    "No agents",
  );
  await waitForAnimations(page);
  await machines.screenshot({ path: `${SHOTS}/04-settings-machines.png` });

  // Keyboard path to Forget, then confirm.
  const forget = machines.getByRole("button", {
    name: "Forget machine alpha-box",
  });
  await forget.focus();
  await page.keyboard.press("Enter");
  const confirm = page.getByTestId("forget-machine-dialog");
  await expect(confirm).toContainText("Forget alpha-box?");
  await waitForAnimations(page);
  await confirm.screenshot({ path: `${SHOTS}/05-forget-confirm.png` });
  await confirm.getByTestId("forget-machine-confirm").click();

  await expect(machines.getByTestId(`machine-row-${ALPHA}`)).toHaveCount(0);
  await expect(machines.getByTestId("settings-machines-notice")).toContainText(
    "alpha-box was forgotten.",
  );
  expect(await commandPayloads(page, "forget_host")).toEqual([
    { hostPubkey: ALPHA },
  ]);
});
