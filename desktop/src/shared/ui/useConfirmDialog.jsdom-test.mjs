import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

const { act, cleanup, fireEvent, render, waitFor } = await import(
  "@testing-library/react"
);
const { createElement } = await import("react");
const { useConfirmDialog } = await import("./useConfirmDialog.tsx");

afterEach(cleanup);

const REQUEST = {
  title: "Delete Example?",
  description: "This cannot be undone.",
  details: ["Removes it from #general"],
  confirmLabel: "Delete",
  destructive: true,
};

function mount() {
  let api;
  function Surface() {
    api = useConfirmDialog();
    return api.confirmDialog;
  }
  const view = render(createElement(Surface));
  return { view, confirm: (request) => api.confirm(request) };
}

const byTestId = (id) => document.querySelector(`[data-testid="${id}"]`);

async function openWith(confirm, request = REQUEST) {
  let answer;
  act(() => {
    answer = confirm(request);
  });
  await waitFor(() => assert.ok(byTestId("confirm-dialog")));
  return { answer };
}

test("the confirm action resolves true and shows the request", async () => {
  const { confirm } = mount();
  const { answer } = await openWith(confirm);
  const dialog = byTestId("confirm-dialog");
  assert.equal(dialog.getAttribute("role"), "alertdialog");
  assert.match(dialog.textContent, /Delete Example\?/);
  assert.match(dialog.textContent, /This cannot be undone\./);
  // The affected items are part of what assistive tech announces.
  const description = document.getElementById(
    dialog.getAttribute("aria-describedby"),
  );
  assert.match(description.textContent, /This cannot be undone\./);
  assert.match(description.textContent, /Removes it from #general/);
  act(() => fireEvent.click(byTestId("confirm-dialog-action")));
  assert.equal(await answer, true);
  await waitFor(() => assert.equal(byTestId("confirm-dialog"), null));
});

test("cancel and Escape resolve false", async () => {
  const { confirm } = mount();
  let { answer } = await openWith(confirm);
  act(() => fireEvent.click(byTestId("confirm-dialog-cancel")));
  assert.equal(await answer, false);

  ({ answer } = await openWith(confirm));
  act(() =>
    fireEvent.keyDown(byTestId("confirm-dialog"), {
      key: "Escape",
      code: "Escape",
    }),
  );
  assert.equal(await answer, false);
});

test("a newer request and unmounting both settle the open one as false", async () => {
  const { confirm, view } = mount();
  const { answer: first } = await openWith(confirm);
  const { answer: second } = await openWith(confirm, {
    ...REQUEST,
    title: "Second?",
  });
  assert.equal(await first, false);
  assert.match(byTestId("confirm-dialog").textContent, /Second\?/);
  view.unmount();
  assert.equal(await second, false);
});
