import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const PUBKEY = "cd".repeat(32);
const calls = [];
const rawAgent = {
  pubkey: PUBKEY,
  name: "Remote Scout",
  persona_id: null,
  relay_url: "wss://relay.example",
  acp_command: "acp",
  agent_command: "agent",
  agent_args: [],
  mcp_command: "mcp",
  turn_timeout_seconds: 60,
  idle_timeout_seconds: 60,
  max_turn_duration_seconds: 60,
  parallelism: 1,
  system_prompt: null,
  model: null,
  status: "deployed",
  pid: null,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  last_started_at: null,
  last_stopped_at: null,
  last_exit_code: null,
  last_error: null,
  log_path: "/tmp/log",
  start_on_app_launch: false,
  backend: { type: "provider", id: "ssh-host", config: {} },
  backend_agent_id: "ssh-host-1",
  provider_policy_pending: true,
  respond_to: "anyone",
  respond_to_allowlist: [],
};
const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    if (command === "start_managed_agent") {
      return Promise.resolve({ ...rawAgent, provider_policy_pending: false });
    }
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
const { TooltipProvider } = await import("../../../shared/ui/tooltip.tsx");
const { fromRawManagedAgent } = await import("../../../shared/api/tauri.ts");
const { AccessPendingBadge } = await import("./AccessPendingBadge.tsx");

test("the summary mapping carries the pending access redeploy", () => {
  assert.equal(fromRawManagedAgent(rawAgent).providerPolicyPending, true);
  const { provider_policy_pending: _omitted, ...legacy } = rawAgent;
  assert.equal(fromRawManagedAgent(legacy).providerPolicyPending, false);
});

test("Retry redeploys the agent through Start and owns one labelled action", async () => {
  calls.length = 0;
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const agent = fromRawManagedAgent(rawAgent);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: new QueryClient() },
        React.createElement(
          TooltipProvider,
          null,
          React.createElement(AccessPendingBadge, { agent }),
        ),
      ),
    );
  });

  const badge = container.querySelector('[data-testid="agent-access-pending"]');
  assert.ok(badge?.textContent?.includes("Access change pending"));
  const buttons = container.querySelectorAll("button");
  assert.equal(buttons.length, 1, "Retry is the only action");
  const retry = buttons[0];
  assert.equal(
    retry.getAttribute("aria-label"),
    "Retry applying the access change to Remote Scout",
  );

  await act(async () => {
    retry.click();
  });
  for (let i = 0; i < 20 && calls.length === 0; i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
  assert.deepEqual(
    calls.map((call) => [call.command, call.args?.pubkey]),
    [["start_managed_agent", PUBKEY]],
  );
  await act(async () => root.unmount());
  container.remove();
});

test("the agents row shows the pending badge only for a remote agent with a pending policy", async () => {
  const rowSource = await readFile(
    new URL("./ManagedAgentRow.tsx", import.meta.url),
    "utf8",
  );
  assert.match(
    rowSource,
    /agent\.providerPolicyPending && !isLocal \? \(\s*<AccessPendingBadge agent=\{agent\} \/>/,
  );
});
