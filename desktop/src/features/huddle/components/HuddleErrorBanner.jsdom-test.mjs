import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { HuddleErrorBanner } = await import("./HuddleErrorBanner.tsx");

const LONG_ERROR =
  "Huddles run in the main Buzz window. Switch to the main window to start or join a huddle in this community, then try again.";

test("the huddle error is shown in full with one labelled dismiss action", async () => {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  let dismissed = 0;
  await act(async () => {
    root.render(
      React.createElement(HuddleErrorBanner, {
        message: LONG_ERROR,
        onDismiss: () => {
          dismissed += 1;
        },
      }),
    );
  });

  const banner = container.querySelector('[role="alert"]');
  assert.ok(banner, "announced as an alert");
  assert.equal(banner.querySelector(".truncate"), null, "never truncated");
  assert.equal(banner.querySelector("[title]"), null, "no duplicate title");
  const message = banner.querySelector("span");
  assert.equal(message.textContent, LONG_ERROR);

  const buttons = banner.querySelectorAll("button");
  assert.equal(buttons.length, 1);
  assert.equal(buttons[0].getAttribute("aria-label"), "Dismiss error");
  assert.equal(
    buttons[0].querySelector('[aria-hidden="true"]')?.textContent,
    "✕",
    "the glyph is not read alongside the label",
  );
  await act(async () => buttons[0].click());
  assert.equal(dismissed, 1);

  await act(async () => root.unmount());
  container.remove();
});

test("the huddle bar renders its error through the banner", async () => {
  const source = await readFile(
    new URL("./HuddleBar.tsx", import.meta.url),
    "utf8",
  );
  assert.match(
    source,
    /<HuddleErrorBanner\s+message=\{huddleError\}\s+onDismiss=\{clearHuddleError\}/,
  );
  assert.doesNotMatch(source, /max-w-\[220px\] truncate">\{huddleError\}/);
});

// HuddleContext rethrows start/join failures and does not toast itself: every
// caller already shows the formatted error as a toast, so a context toast
// would show each failure twice.
const CALLERS = [
  ["../../channels/ui/ChannelMembersBar.tsx", "start"],
  ["../../messages/ui/WaveMessageAttachment.tsx", "start"],
  ["../../profile/ui/useProfileInteractionActions.ts", "start"],
  ["./HuddleIndicator.tsx", "join"],
  ["./HuddleAttachment.tsx", "join"],
];

for (const [path, action] of CALLERS) {
  test(`${path} toasts a failed huddle ${action}`, async () => {
    const source = await readFile(new URL(path, import.meta.url), "utf8");
    assert.ok(
      source.includes(
        `toast.error(formatHuddleActionError(error, "${action}"))`,
      ) ||
        source.includes(`toast.error(formatHuddleActionError(e, "${action}"))`),
    );
  });
}

test("the huddle context leaves toasts to its callers", async () => {
  const source = await readFile(
    new URL("../HuddleContext.tsx", import.meta.url),
    "utf8",
  );
  assert.doesNotMatch(source, /toast\.error/);
});
