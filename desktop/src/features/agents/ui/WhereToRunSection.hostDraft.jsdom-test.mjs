import assert from "node:assert/strict";
import test from "node:test";

const HOST_PUBKEY = "a1".repeat(32);
const tauriMock = {
  invoke(command) {
    if (command === "list_agent_hosts") {
      return Promise.resolve([
        {
          pubkey: HOST_PUBKEY,
          name: "Devbox",
          os: "linux",
          arch: "x86_64",
          relay_url: "wss://relay.example",
          added_at: "2026-01-01T00:00:00Z",
          status: null,
        },
      ]);
    }
    if (command === "get_presence") {
      return Promise.resolve({ [HOST_PUBKEY]: "online" });
    }
    if (command === "discover_backend_providers") return Promise.resolve([]);
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback() {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { WhereToRunSection } = await import("./WhereToRunSection.tsx");
const { runDraftFromRequest } = await import("../agentManagement.ts");

test("a machine draft shows the machine and its prefilled folder", async () => {
  const { draft } = runDraftFromRequest(
    { runOnHost: "devbox", hostWorkdir: "~/code/app" },
    [],
    [{ pubkey: HOST_PUBKEY, name: "Devbox" }],
  );
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(WhereToRunSection, {
          draft,
          isPending: false,
          onDraftChange: () => {},
        }),
      ),
    );
  });
  let folder = null;
  for (let i = 0; i < 40 && !folder; i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
    folder = container.querySelector(
      "[data-testid='where-to-run-host-workdir']",
    );
  }
  assert.ok(folder, "the Folder field renders for the drafted machine");
  assert.equal(folder.value, "~/code/app");
  const label = container.querySelector(`label[for='${folder.id}']`);
  assert.equal(label?.textContent, "Folder on Devbox");
  await act(async () => root.unmount());
  container.remove();
});
