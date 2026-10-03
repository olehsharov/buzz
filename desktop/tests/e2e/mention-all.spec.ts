import { expect, test, type Page } from "@playwright/test";
import { waitForAnimations } from "../helpers/animations";
import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

// `general` roster: the viewer (owner), Alice (an agent in the mock
// directory), Bob, and a member whose profile alone marks it as an agent.
// `@all` must reach exactly the one human besides the sender.
const HUMANS = [TEST_IDENTITIES.bob.pubkey];
const MARKER = ["buzz:mention-group", "all"];
const OUT = "test-results/mention-all";

type SentEvent = { content: string; pubkeys: string[]; tags: string[][] };

async function sent(page: Page, content: string): Promise<SentEvent[]> {
  return page.evaluate((content) => {
    const signed = (window.__BUZZ_E2E_SIGNED_EVENTS__ ?? [])
      .filter((event) => event.content === content)
      .map((event) => ({
        content: event.content,
        pubkeys: event.tags
          .filter((tag) => tag[0] === "p")
          .map((tag) => tag[1])
          .sort(),
        tags: event.tags,
      }));
    if (signed.length > 0) return signed;
    // Sends that need the acknowledged REST path go through native IPC.
    return (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((call) => call.command === "send_channel_message")
      .map(
        (call) =>
          call.payload as {
            content: string;
            mentionPubkeys?: string[];
            mentionTags?: string[][] | null;
          },
      )
      .filter((payload) => payload.content === content)
      .map((payload) => ({
        content: payload.content,
        pubkeys: [...(payload.mentionPubkeys ?? [])].sort(),
        tags: payload.mentionTags ?? [],
      }));
  }, content);
}

async function open(page: Page, channel: string, extra?: string[]) {
  await installMockBridge(
    page,
    extra ? { extraChannelMembers: { [channel]: extra } } : undefined,
  );
  await page.goto("/");
  await page.getByTestId(`channel-${channel}`).click();
  await expect(page.getByTestId("chat-title")).toHaveText(channel);
}

test("picking @all notifies every human member and renders one pill", async ({
  page,
}) => {
  await open(page, "general");
  const input = page.getByTestId("message-input");
  await input.click();
  await page.keyboard.type("@al");
  const option = page.getByTestId("mention-suggestion-group-all");
  await expect(option).toBeVisible();
  await expect(option).toContainText("@all");
  await expect(option).toContainText("Notify 1 person in this channel");
  await expect(option.getByRole("button")).toHaveAccessibleName(
    "Mention @all: Notify 1 person in this channel",
  );
  await waitForAnimations(page);
  await page
    .getByTestId("mention-autocomplete")
    .screenshot({ path: `${OUT}/01-autocomplete.png` });

  await option.getByRole("button").click();
  await page.keyboard.type("standup moved to 3pm");
  const content = "@all standup moved to 3pm";
  await expect(input).toHaveText(content);
  await page.keyboard.press("Enter");

  await expect
    .poll(() => sent(page, content))
    .toEqual([expect.objectContaining({ pubkeys: [...HUMANS].sort() })]);
  const [event] = await sent(page, content);
  expect(event.tags.filter((tag) => tag[0] === MARKER[0])).toEqual([MARKER]);

  const row = page
    .getByTestId("message-timeline")
    .getByTestId("message-row")
    .filter({ hasText: "standup moved to 3pm" })
    .last();
  const pill = row.locator('[data-mention-kind="group"]');
  await expect(pill).toHaveCount(1);
  await expect(pill).toHaveText("all");
  await expect(pill).toHaveAttribute(
    "aria-label",
    "@all, everyone in this channel",
  );
  // One group pill, not one chip per recipient.
  await expect(row.locator("[data-mention]")).toHaveCount(1);
  await waitForAnimations(page);
  await row.screenshot({ path: `${OUT}/02-rendered-message.png` });

  // Editing keeps the original marker (the edit itself never re-notifies)
  // and the edit composer does not offer the group.
  await row.hover();
  await row.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("menuitem", { name: "Edit message" }).click();
  await expect(page.getByTestId("edit-target")).toBeVisible();
  await input.press("End");
  await page.keyboard.type(" @al");
  await expect(page.getByTestId("mention-autocomplete")).toBeVisible();
  await expect(page.getByTestId("mention-suggestion-group-all")).toHaveCount(0);
  await input.fill("@all standup moved to 3pm today");
  await page.getByTestId("send-message").click();
  await expect(page.getByTestId("edit-target")).toHaveCount(0);
  const edited = page
    .getByTestId("message-timeline")
    .getByTestId("message-row")
    .filter({ hasText: "standup moved to 3pm today" })
    .last();
  await expect(edited.locator('[data-mention-kind="group"]')).toHaveCount(1);
  const edits = await page.evaluate(() =>
    (window.__BUZZ_E2E_COMMAND_LOG__ ?? [])
      .filter((call) => call.command === "edit_message")
      .map(
        (call) =>
          (call.payload as { input: { mentionPubkeys: string[] } }).input,
      ),
  );
  expect(edits.at(-1)?.mentionPubkeys).toEqual([]);
});

test("a typed @all resolves to the group like a picked one", async ({
  page,
}) => {
  await open(page, "general");
  const input = page.getByTestId("message-input");
  await input.fill("@all typed hello");
  await input.press("Escape");
  await page.getByTestId("send-message").click();
  await expect
    .poll(() => sent(page, "@all typed hello"))
    .toEqual([expect.objectContaining({ pubkeys: [...HUMANS].sort() })]);
});

test("@all over the cap is disabled with a reason and blocks typed sends", async ({
  page,
}) => {
  // 50 extra humans + Bob = 51 people besides the sender.
  const extra = Array.from({ length: 50 }, (_, index) =>
    (index + 1).toString(16).padStart(64, "7"),
  );
  await open(page, "general", extra);
  const input = page.getByTestId("message-input");
  await input.click();
  await page.keyboard.type("@al");
  const option = page.getByTestId("mention-suggestion-group-all");
  await expect(option).toContainText(
    "@all is limited to channels with up to 50 people",
  );
  const button = option.getByRole("button");
  await expect(button).toHaveAttribute("aria-disabled", "true");
  // aria-disabled is not native `disabled`: force the pointer press through
  // Playwright's actionability gate to prove the handler itself refuses it.
  await button.click({ force: true });
  await expect(input).toHaveText("@al");
  // Arrow onto the entry, then Enter and Tab: still not inserted.
  const index = Number(
    await option.getAttribute("data-mention-suggestion-index"),
  );
  for (let step = 0; step < index; step += 1) {
    await page.keyboard.press("ArrowDown");
  }
  await expect(option).toHaveClass(/bg-accent text-accent-foreground/);
  await page.keyboard.press("Enter");
  await expect(input).toHaveText("@al");
  await page.keyboard.press("Tab");
  await expect(input).toHaveText("@al");
  await expect(option).toBeVisible();
  await waitForAnimations(page);
  await page
    .getByTestId("mention-autocomplete")
    .screenshot({ path: `${OUT}/03-autocomplete-over-cap.png` });

  await input.fill("@all over the cap");
  await input.press("Escape");
  await page.getByTestId("send-message").click();
  await expect(
    page
      .getByText(
        "@all is limited to channels with up to 50 people. This channel has 51.",
      )
      .first(),
  ).toBeVisible();
  await expect(input).toHaveText("@all over the cap");
  expect(await sent(page, "@all over the cap")).toEqual([]);
});

test("DMs never offer @all", async ({ page }) => {
  await open(page, "alice-tyler");
  const input = page.getByTestId("message-input");
  await input.click();
  await page.keyboard.type("@");
  await expect(page.getByTestId("mention-autocomplete")).toBeVisible();
  await expect(page.getByTestId("mention-suggestion-group-all")).toHaveCount(0);
  await page.keyboard.type("all");
  await expect(page.getByTestId("mention-suggestion-group-all")).toHaveCount(0);
});
