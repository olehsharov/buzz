import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";
import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});
const PK = "a".repeat(64);
const SIBLING = "b".repeat(64);
const agent = {
  pubkey: PK,
  name: "Remote",
  personaId: "persona",
  status: "deployed",
  backend: { type: "provider", id: "fixture", config: {} },
  backendAgentId: "receipt",
};
const channel = { id: "channel", name: "agents", memberPubkeys: [PK] };
const directory = [
  { pubkey: PK, channels: ["agents"], channelIds: ["channel"] },
];
let act,
  render,
  cleanup,
  waitFor,
  createElement,
  QueryClient,
  QueryClientProvider;
let useAgentAvailabilityLookup,
  useManagedAgentActions,
  useProfileAgentDeletion,
  CommunitiesProvider;
let deleteManagedAgentWithRules, deleteManagedAgent, relayClient, originals;
let connection, listeners, handlers, commands, confirms, clients;
let confirmAnswer, dialogObserver, fireEvent, useConfirmDialog;

before(async () => {
  Object.assign(globalThis, {
    window: dom.window,
    localStorage: dom.window.localStorage,
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: dom.window.navigator,
  });
  // The in-app confirmation dialog (Radix) needs the rest of the DOM API.
  for (const key of Object.getOwnPropertyNames(dom.window)) {
    if (!(key in globalThis)) {
      try {
        globalThis[key] = dom.window[key];
      } catch {}
    }
  }
  globalThis.Event = dom.window.Event;
  globalThis.CustomEvent = dom.window.CustomEvent;
  dom.window.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      commands.push([command, args]);
      if (handlers.has(command)) return handlers.get(command)(args);
      throw new Error(`Unexpected IPC: ${command}`);
    },
    transformCallback: () => 1,
  };
  ({ act, render, cleanup, waitFor, fireEvent } = await import(
    "@testing-library/react"
  ));
  ({ useConfirmDialog } = await import(
    "../../../shared/ui/useConfirmDialog.tsx"
  ));
  ({ createElement } = await import("react"));
  ({ QueryClient, QueryClientProvider } = await import(
    "@tanstack/react-query"
  ));
  ({ CommunitiesProvider } = await import(
    "../../communities/useCommunities.tsx"
  ));
  ({ useAgentAvailabilityLookup } = await import("./useAgentAvailability.ts"));
  ({ useManagedAgentActions } = await import(
    "../ui/useManagedAgentActions.ts"
  ));
  ({ useProfileAgentDeletion } = await import(
    "../../profile/ui/UserProfilePanelDeletion.ts"
  ));
  ({ deleteManagedAgentWithRules } = await import(
    "./managedAgentControlActions.ts"
  ));
  ({ deleteManagedAgent } = await import("../../../shared/api/tauri.ts"));
  ({ relayClient } = await import("../../../shared/api/relayClient.ts"));
  originals = {
    getConnectionState: relayClient.getConnectionState,
    subscribeToConnectionState: relayClient.subscribeToConnectionState,
  };
  relayClient.getConnectionState = () => connection;
  relayClient.subscribeToConnectionState = (listener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  };
});

afterEach(() => {
  cleanup();
  for (const client of clients ?? []) {
    client.cancelQueries();
    client.clear();
  }
});
after(() => {
  Object.assign(relayClient, originals);
  dom.window.close();
});

function setup() {
  clients = [];
  commands = [];
  confirms = [];
  connection = "connected";
  listeners = new Set();
  handlers = new Map([
    ["get_presence", () => ({ [PK]: "online" })],
    ["delete_managed_agent", () => null],
    ["remove_channel_member", () => null],
    ["send_channel_message", () => ({ event_id: "event", created_at: 0 })],
    ["list_managed_agents", () => []],
    ["get_relay_agents", () => []],
    ["list_available_acp_runtimes", () => []],
    ["get_channels", () => []],
    ["plugin:event|listen", () => 1],
    ["plugin:event|unlisten", () => null],
  ]);
  confirmAnswer = true;
}

/** Direct-call stand-in for the in-app dialog. */
const testConfirm = async (request) => {
  confirms.push(request.description);
  return confirmAnswer;
};

/** Run a hook operation that may open the in-app dialog. Awaiting it inside
 * `act` would deadlock: act holds the dialog's render until the callback
 * settles, and the callback waits on the dialog. `waitFor` polls with act
 * disabled, so the dialog renders and is answered. */
async function runToCompletion(start) {
  let operation;
  act(() => {
    operation = start();
  });
  let outcome;
  operation.then(
    (value) => {
      outcome = { value };
    },
    (error) => {
      outcome = { error };
    },
  );
  await waitFor(() => assert.ok(outcome, "operation still pending"), {
    timeout: 5000,
  });
  if (outcome.error) throw outcome.error;
  return outcome.value;
}

/** Answer the real in-app dialog the hook surfaces render, recording its
 * text, so the hook paths exercise `useConfirmDialog` end to end. */
function answerDialogsAutomatically() {
  dialogObserver?.disconnect();
  dialogObserver = new dom.window.MutationObserver(() => {
    const dialog = dom.window.document.querySelector(
      '[data-testid="confirm-dialog"]',
    );
    if (!dialog || dialog.dataset.answered) return;
    dialog.dataset.answered = "true";
    confirms.push(dialog.textContent);
    const button = dialog.querySelector(
      confirmAnswer
        ? '[data-testid="confirm-dialog-action"]'
        : '[data-testid="confirm-dialog-cancel"]',
    );
    queueMicrotask(() => fireEvent.click(button));
  });
  dialogObserver.observe(dom.window.document.body, {
    childList: true,
    subtree: true,
  });
}

function mount(
  owner,
  { agents = [agent], keys = [PK], seedChannels = true } = {},
) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0, staleTime: Infinity },
      mutations: { retry: false, gcTime: 0 },
    },
  });
  clients.push(client);
  client.setQueryData(["managed-agents"], agents);
  client.setQueryData(["relay-agents"], directory);
  if (seedChannels) client.setQueryData(["channels"], [channel]);
  client.setQueryData(["globalAgentConfig"], { env_vars: {} });
  let current;
  function AgentsSurface() {
    current = useManagedAgentActions();
    return current.confirmDialog;
  }
  function ProfileSurface() {
    const availability = useAgentAvailabilityLookup(keys);
    const { confirm, confirmDialog } = useConfirmDialog();
    const deletion = useProfileAgentDeletion({
      confirm,
      channels: [channel],
      managedAgents: agents,
      managedAgent: agents[0],
      relayAgents: agents.map((row) => ({
        ...directory[0],
        pubkey: row.pubkey,
      })),
      getAvailability: availability.getAvailability,
      deleteManagedAgent: ({ pubkey, forceRemoteDelete }) =>
        deleteManagedAgent(pubkey, forceRemoteDelete),
    });
    current = { ...availability, ...deletion };
    return confirmDialog;
  }
  const Surface = owner === "agents" ? AgentsSurface : ProfileSurface;
  answerDialogsAutomatically();
  render(
    createElement(
      QueryClientProvider,
      { client },
      createElement(CommunitiesProvider, null, createElement(Surface)),
    ),
  );
  return { client, current: () => current };
}

function effects() {
  return commands.filter(([name]) =>
    [
      "send_channel_message",
      "delete_managed_agent",
      "remove_channel_member",
    ].includes(name),
  );
}

for (const owner of ["agents", "profile"]) {
  for (const scenario of [
    "online",
    "away",
    "offline",
    "missing",
    "pending",
    "failed-online",
    "failed-offline",
    "disconnected-online",
    "disconnected-offline",
  ]) {
    test(`${owner} deletion uses resolved ${scenario} at the production hook/IPC boundary`, async () => {
      setup();
      const warm = scenario.endsWith("-offline") ? "offline" : "online";
      handlers.set("get_presence", () => {
        if (scenario === "pending") return new Promise(() => {});
        if (scenario === "missing") return {};
        return { [PK]: scenario.includes("-") ? warm : scenario };
      });
      const surface = mount(owner);
      const key = ["presence", PK];
      if (scenario !== "pending") {
        await waitFor(() =>
          assert.equal(surface.client.getQueryState(key)?.status, "success"),
        );
      }
      if (scenario.startsWith("failed")) {
        handlers.set("get_presence", () =>
          Promise.reject("relay unreachable: request timed out"),
        );
        await act(() =>
          surface.client.invalidateQueries({ queryKey: key, exact: true }),
        );
        assert.equal(surface.client.getQueryState(key).status, "error");
        assert.deepEqual(surface.client.getQueryData(key), { [PK]: warm });
      }
      if (scenario.startsWith("disconnected")) {
        await act(async () => {
          connection = "disconnected";
          for (const listener of listeners) listener(connection);
        });
        assert.deepEqual(surface.client.getQueryData(key), { [PK]: warm });
      }
      const unknown = scenario.includes("-") || scenario === "pending";
      await waitFor(() =>
        assert.equal(
          surface.current().getAvailability(PK),
          unknown ? undefined : scenario === "missing" ? "offline" : scenario,
        ),
      );
      commands.length = 0;
      await runToCompletion(() =>
        owner === "agents"
          ? surface.current().handleDelete(PK)
          : surface.current().deleteManagedAgentRecord(agent),
      );
      const shouldShutdown = scenario !== "offline" && scenario !== "missing";
      assert.deepEqual(
        effects().map(([name]) => name),
        [
          ...(shouldShutdown ? ["send_channel_message"] : []),
          "delete_managed_agent",
        ],
      );
      if (shouldShutdown) {
        assert.equal(effects()[0][1].content, "!shutdown");
        assert.deepEqual(effects()[0][1].mentionPubkeys, [PK]);
      }
      assert.deepEqual(
        effects().find(([name]) => name === "delete_managed_agent")[1],
        {
          pubkey: PK,
          forceRemoteDelete: true,
        },
      );
      if (owner === "agents") {
        assert.equal(confirms.length, 1);
        if (unknown) {
          assert.match(confirms[0], /availability is unknown/);
          assert.doesNotMatch(confirms[0], /offline/i);
        } else if (!shouldShutdown) assert.match(confirms[0], /is offline/);
      } else
        assert.deepEqual(confirms, [], "profile already obtained confirmation");
    });
  }
}

test("reader retained across an await sees errors/disconnect, not cached success; unqueried siblings stay unknown", async () => {
  setup();
  const surface = mount("profile");
  await waitFor(() =>
    assert.equal(surface.current().getAvailability(PK), "online"),
  );
  const retainedReader = surface.current().getAvailability;
  assert.equal(retainedReader(SIBLING), undefined);
  handlers.set("get_presence", () => Promise.reject("failed"));
  await act(() =>
    surface.client.invalidateQueries({ queryKey: ["presence", PK] }),
  );
  assert.equal(retainedReader(PK), undefined);
  await act(async () =>
    surface.client.setQueryData(["presence", PK], { [PK]: "online" }),
  );
  connection = "reconnecting"; // even before the next React connection render
  assert.equal(retainedReader(PK), undefined);
});

for (const owner of ["agents", "profile"]) {
  test(`${owner} unknown shutdown failure preserves record and channel membership`, async () => {
    setup();
    handlers.set("get_presence", () => Promise.reject("failed"));
    handlers.set("send_channel_message", () =>
      Promise.reject(new Error("shutdown refused")),
    );
    const surface = mount(owner);
    await waitFor(() =>
      assert.equal(
        surface.client.getQueryState(["presence", PK])?.status,
        "error",
      ),
    );
    await act(async () => {
      if (owner === "agents") await surface.current().handleDelete(PK);
      else
        await assert.rejects(
          surface.current().deleteManagedAgentRecord(agent),
          /shutdown refused/,
        );
    });
    assert.deepEqual(
      effects().map(([name]) => name),
      ["send_channel_message"],
    );
    assert.deepEqual(confirms, []);
    if (owner === "agents")
      assert.equal(surface.current().actionErrorMessage, "shutdown refused");
  });
}

test("unknown waits for shutdown before confirmation/delete; cancellation retains record", async () => {
  setup();
  let release;
  handlers.set(
    "send_channel_message",
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  confirmAnswer = false;
  const operation = deleteManagedAgentWithRules({
    confirm: testConfirm,
    agent,
    channels: [channel],
    relayAgents: directory,
    getAvailability: () => undefined,
    deleteManagedAgent: ({ pubkey, forceRemoteDelete }) =>
      deleteManagedAgent(pubkey, forceRemoteDelete),
  });
  await waitFor(() => assert.equal(typeof release, "function"));
  assert.deepEqual(confirms, []);
  assert.equal(effects().length, 1);
  release({ event_id: "event" });
  assert.deepEqual(await operation, { cancelled: true });
  assert.match(confirms[0], /availability is unknown/);
  assert.equal(effects().length, 1);
});

test("no channel warns without claiming process state; local deletion ignores presence", async () => {
  setup();
  const remove = ({ pubkey, forceRemoteDelete }) =>
    deleteManagedAgent(pubkey, forceRemoteDelete);
  await deleteManagedAgentWithRules({
    confirm: testConfirm,
    agent,
    channels: [],
    relayAgents: [],
    getAvailability: () => undefined,
    deleteManagedAgent: remove,
  });
  assert.match(confirms[0], /may still be running/);
  assert.doesNotMatch(confirms[0], /will keep running|offline/i);
  assert.deepEqual(
    effects().map(([name]) => name),
    ["delete_managed_agent"],
  );
  commands.length = 0;
  confirms.length = 0;
  await deleteManagedAgentWithRules({
    confirm: testConfirm,
    agent: { ...agent, backend: { type: "local" } },
    channels: [],
    relayAgents: [],
    getAvailability: () => {
      throw new Error("must not consult presence");
    },
    deleteManagedAgent: remove,
  });
  assert.deepEqual(effects(), [
    ["delete_managed_agent", { pubkey: PK, forceRemoteDelete: null }],
  ]);
  assert.deepEqual(confirms, []);
});

test("Agents deletion rechecks availability after channel discovery, not the click-time snapshot", async () => {
  setup();
  let releaseChannels;
  handlers.set(
    "get_channels",
    () =>
      new Promise((resolve) => {
        releaseChannels = resolve;
      }),
  );
  handlers.set("get_presence", () => ({ [PK]: "offline" }));
  const surface = mount("agents", { seedChannels: false });
  await waitFor(() =>
    assert.equal(surface.current().getAvailability(PK), "offline"),
  );
  let operation;
  await act(async () => {
    operation = surface.current().handleDelete(PK);
  });
  await waitFor(() => assert.equal(typeof releaseChannels, "function"));
  handlers.set("get_presence", () => Promise.reject("failed"));
  await act(() =>
    surface.client.invalidateQueries({ queryKey: ["presence", PK] }),
  );
  assert.deepEqual(effects(), []);
  await runToCompletion(() => {
    releaseChannels({ hash: "empty", channels: [], last_messages: {} });
    return operation;
  });
  assert.deepEqual(
    effects().map(([name]) => name),
    ["send_channel_message", "delete_managed_agent"],
  );
  assert.match(confirms[0], /availability is unknown/);
});

test("profile persona deletion cannot infer Offline for an unqueried sibling", async () => {
  setup();
  handlers.set("get_presence", () => ({}));
  const surface = mount("profile", {
    agents: [agent, { ...agent, pubkey: SIBLING }],
    keys: [PK],
  });
  await waitFor(() =>
    assert.equal(surface.current().getAvailability(PK), "offline"),
  );
  assert.equal(surface.current().getAvailability(SIBLING), undefined);
  await runToCompletion(() =>
    surface.current().deleteManagedAgentsForPersona({ id: "persona" }),
  );
  const requests = effects().filter(
    ([name]) => name === "send_channel_message",
  );
  assert.equal(requests.length, 1);
  assert.deepEqual(requests[0][1].mentionPubkeys, [SIBLING]);
  assert.match(confirms[0], /is offline/);
  assert.match(confirms[1], /availability is unknown/);
});

test("successful cached snapshot remains authoritative during refetch; only settled error revokes it", async () => {
  setup();
  const surface = mount("profile");
  await waitFor(() =>
    assert.equal(surface.current().getAvailability(PK), "online"),
  );
  let rejectRead;
  handlers.set(
    "get_presence",
    () =>
      new Promise((_, reject) => {
        rejectRead = reject;
      }),
  );
  let refresh;
  await act(async () => {
    refresh = surface.client.invalidateQueries({ queryKey: ["presence", PK] });
  });
  assert.equal(surface.current().getAvailability(PK), "online");
  await act(async () => {
    rejectRead("failed");
    await refresh;
  });
  assert.equal(surface.current().getAvailability(PK), undefined);
});

for (const owner of ["agents", "profile"]) {
  test(`${owner} deletion leaves channels to the backend and reports any it kept`, async () => {
    setup();
    handlers.set("get_presence", () => ({ [PK]: "offline" }));
    handlers.set("delete_managed_agent", () => ({
      channels: {
        removed: [{ id: "channel", name: "agents" }],
        failed: [
          {
            channel_id: "locked",
            channel_name: "ops",
            error: "relay rejected event: not a member",
          },
        ],
        lookup_error: null,
      },
    }));
    const surface = mount(owner);
    await waitFor(() =>
      assert.equal(surface.current().getAvailability(PK), "offline"),
    );
    commands.length = 0;
    const result = await runToCompletion(() =>
      owner === "agents"
        ? surface.current().handleDelete(PK)
        : surface.current().deleteManagedAgentRecord(agent),
    );
    // The frontend no longer publishes removals itself.
    assert.deepEqual(
      effects().map(([name]) => name),
      ["delete_managed_agent"],
    );
    if (owner === "agents") {
      await waitFor(() =>
        assert.match(
          surface.current().actionErrorMessage ?? "",
          /Remote is still a member of #ops \(relay rejected event: not a member\)/,
        ),
      );
    } else {
      assert.deepEqual(result.channelCleanup.failed, [
        {
          channelId: "locked",
          channelName: "ops",
          error: "relay rejected event: not a member",
        },
      ]);
    }
  });
}
