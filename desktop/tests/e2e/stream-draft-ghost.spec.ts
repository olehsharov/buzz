import { expect, type Page, test } from "@playwright/test";

import { KIND_STREAM_DRAFT } from "../../src/shared/constants/kinds";
import { waitForAnimations } from "../helpers/animations";
import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

const CHANNEL = "agents";
const STREAM = "7f1c2d3e-4b5a-4c6d-8e9f-0a1b2c3d4e5f";
const SCREENSHOT_DIR = "test-results/stream-drafts";
const ALICE = TEST_IDENTITIES.alice.pubkey;

type StreamDraftInput = {
  seq: number;
  status: string;
  content?: string;
  label?: string;
  stream?: string;
  threadRootId?: string;
  threadParentId?: string;
};

async function emitDraft(page: Page, input: StreamDraftInput) {
  await page.evaluate(
    ({ channelName, pubkey, draft, stream }) => {
      window.__BUZZ_E2E_EMIT_MOCK_STREAM_DRAFT__?.({
        channelName,
        pubkey,
        stream: draft.stream ?? stream,
        ...draft,
      });
    },
    { channelName: CHANNEL, pubkey: ALICE, draft: input, stream: STREAM },
  );
}

async function waitForStreamDraftSubscription(page: Page) {
  await expect
    .poll(() =>
      page.evaluate(
        ({ channelName, kind }) =>
          window.__BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
            channelName,
            kind,
          }) ?? false,
        { channelName: CHANNEL, kind: KIND_STREAM_DRAFT },
      ),
    )
    .toBe(true);
}

async function captureDrafts(page: Page, name: string) {
  await waitForAnimations(page);
  const box = await page.getByTestId("stream-drafts").boundingBox();
  if (!box) throw new Error("stream drafts are not laid out");
  // Include the row above the ghost for context.
  const top = Math.max(0, box.y - 96);
  await page.screenshot({
    path: `${SCREENSHOT_DIR}/${name}.png`,
    clip: {
      x: Math.max(0, box.x - 8),
      y: top,
      width: box.width + 16,
      height: box.y + box.height - top + 8,
    },
  });
}

test.beforeEach(async ({ page }) => {
  await installMockBridge(page);
});

test("live reply ghost: status line, streaming text, and duplicate-free hand-off", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByTestId(`channel-${CHANNEL}`).click();
  await expect(page.getByTestId("chat-title")).toHaveText(CHANNEL);
  await waitForStreamDraftSubscription(page);

  const timeline = page.getByTestId("message-timeline");
  const statusLine = timeline.getByTestId("stream-draft-status");
  const ghost = timeline.getByTestId("stream-draft-row");

  // 1. Status-only frame → enhanced typing line, no message ghost.
  await emitDraft(page, { seq: 1, status: "thinking" });
  await expect(statusLine).toHaveCount(1);
  await expect(statusLine).toContainText("alice is thinking…");
  await expect(ghost).toHaveCount(0);
  await expect(page.getByTestId("stream-draft-live")).toHaveText(
    "alice is replying",
  );
  await captureDrafts(page, "01-status-thinking");

  await emitDraft(page, { seq: 2, status: "tool", label: "Read file" });
  await expect(statusLine).toContainText("alice is running Read file…");

  // 2. Writing frames replace the snapshot; partial fences stay contained.
  await emitDraft(page, {
    seq: 3,
    status: "writing",
    content: "Here is the plan:\n\n```ts\nconst ans",
  });
  await expect(ghost).toHaveCount(1);
  await expect(statusLine).toHaveCount(0);
  await expect(ghost.getByTestId("stream-draft-badge")).toHaveText("Writing…");
  await expect(ghost.locator("pre")).toContainText("const ans");

  await emitDraft(page, {
    seq: 4,
    status: "writing",
    content:
      "Here is the plan:\n\n```ts\nconst answer = 42;\n```\n\nShipping it **now**",
  });
  await expect(ghost.locator("pre")).toContainText("const answer = 42;");
  await expect(ghost.getByTestId("stream-draft-body")).toContainText(
    "Shipping it now",
  );

  // A reordered older frame is ignored.
  await emitDraft(page, { seq: 3, status: "writing", content: "stale" });
  await expect(ghost.getByTestId("stream-draft-body")).toContainText(
    "Shipping it now",
  );
  await expect(ghost.getByTestId("stream-draft-body")).not.toContainText(
    "stale",
  );
  // Per-chunk updates are not announced.
  await expect(page.getByTestId("stream-draft-live")).not.toContainText(
    "finished",
  );
  await captureDrafts(page, "02-ghost-writing");

  // A frame for another scope (a thread) never lands in the channel timeline.
  await emitDraft(page, {
    seq: 1,
    status: "writing",
    content: "thread-only reply",
    stream: "aaaaaaaa-0000-4000-8000-000000000001",
    threadRootId: "f".repeat(64),
    threadParentId: "f".repeat(64),
  });
  await expect(timeline).not.toContainText("thread-only reply");

  // 3. Final kind 9 (autoposted, stream-tagged) replaces the ghost with no
  // commit in which both are on screen and no commit in which neither is.
  await page.evaluate(() => {
    const isFinalRow = (element: Element) =>
      element.textContent?.includes("Shipping it now") ?? false;
    const state = { overlap: 0, gap: 0, sawMessage: false };
    const check = () => {
      const ghostVisible = document.querySelector(
        '[data-testid="stream-draft-row"]',
      );
      const messageVisible = [
        ...document.querySelectorAll('[data-testid="message-row"]'),
      ].some(isFinalRow);
      if (ghostVisible && messageVisible) state.overlap += 1;
      if (!ghostVisible && !messageVisible && !state.sawMessage) {
        state.gap += 1;
      }
      if (messageVisible) state.sawMessage = true;
    };
    const observer = new MutationObserver(check);
    observer.observe(document.body, { childList: true, subtree: true });
    (window as unknown as { __handoff: typeof state }).__handoff = state;
  });

  await page.evaluate(
    ({ pubkey, stream }) => {
      window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName: "agents",
        pubkey,
        content:
          "Here is the plan:\n\n```ts\nconst answer = 42;\n```\n\nShipping it **now**",
        extraTags: [["stream", stream]],
      });
    },
    { pubkey: ALICE, stream: STREAM },
  );

  const finalRows = timeline
    .getByTestId("message-row")
    .filter({ hasText: "Shipping it now" });
  await expect(finalRows).toHaveCount(1);
  await expect(ghost).toHaveCount(0);
  await expect(page.getByTestId("stream-draft-live")).toHaveText(
    "alice finished replying",
  );
  const handoff = await page.evaluate(
    () =>
      (window as unknown as { __handoff: { overlap: number; gap: number } })
        .__handoff,
  );
  expect(handoff).toEqual(expect.objectContaining({ overlap: 0, gap: 0 }));

  // A late frame from the finished stream cannot resurrect the ghost.
  await emitDraft(page, { seq: 9, status: "writing", content: "zombie" });
  await expect(timeline).not.toContainText("zombie");
  await expect(finalRows).toHaveCount(1);
});
