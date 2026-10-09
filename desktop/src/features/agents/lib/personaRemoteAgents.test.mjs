import assert from "node:assert/strict";
import test from "node:test";

import {
  deletePersonaAfterRemoteAgents,
  remoteAgentLocation,
  remoteAgentsOfPersona,
} from "./personaRemoteAgents.ts";

const PERSONA = { id: "persona-1", displayName: "Ops Template" };

function agent(name, overrides = {}) {
  return {
    pubkey: name.toLowerCase().padEnd(64, "0"),
    name,
    personaId: PERSONA.id,
    backend: { type: "local" },
    backendAgentId: null,
    ...overrides,
  };
}

const onMachine = agent("Infra", {
  backend: { type: "host", host_pubkey: "ab".repeat(32) },
  backendAgentId: "ab".repeat(32),
  hostName: "workstation",
});
const viaProvider = agent("Cloud", {
  backend: { type: "provider", id: "blox", config: {} },
  backendAgentId: "receipt",
});
const local = agent("Local");
const undeployedMachine = agent("Parked", {
  backend: { type: "host", host_pubkey: "ab".repeat(32) },
  backendAgentId: null,
});
const otherPersona = agent("Other", { ...onMachine, personaId: "persona-2" });
const AGENTS = [onMachine, viaProvider, local, undeployedMachine, otherPersona];

const cleanup = (id) => ({
  removed: [{ id, name: id }],
  failed: [],
  lookupError: null,
});

test("only the persona's agents deployed elsewhere block, each with where it runs", () => {
  const remote = remoteAgentsOfPersona(AGENTS, PERSONA.id);
  assert.deepEqual(
    remote.map((row) => row.name),
    ["Infra", "Cloud"],
  );
  assert.deepEqual(remote.map(remoteAgentLocation), [
    "Infra is running on workstation",
    "Cloud is deployed through blox",
  ]);
  assert.equal(
    remoteAgentLocation({ ...onMachine, hostName: null }),
    "Infra is running on a paired machine",
  );
});

test("one confirmation removes remote agents first, then deletes the persona", async () => {
  const order = [];
  const result = await deletePersonaAfterRemoteAgents({
    persona: PERSONA,
    managedAgents: AGENTS,
    deleteAgent: async (row) => {
      order.push(`agent ${row.name}`);
      return { channelCleanup: cleanup(row.name) };
    },
    deletePersona: async (id) => {
      order.push(`persona ${id}`);
      return cleanup("persona");
    },
  });
  assert.deepEqual(order, ["agent Infra", "agent Cloud", "persona persona-1"]);
  assert.deepEqual(
    result.channelCleanup.removed.map((channel) => channel.id),
    ["Infra", "Cloud", "persona"],
  );
});

test("a declined machine delete leaves the persona and the rest untouched", async () => {
  const order = [];
  const result = await deletePersonaAfterRemoteAgents({
    persona: PERSONA,
    managedAgents: AGENTS,
    deleteAgent: async (row) => {
      order.push(row.name);
      return { cancelled: true };
    },
    deletePersona: async () => {
      throw new Error("must not delete the persona");
    },
  });
  assert.deepEqual(result, { cancelled: true });
  assert.deepEqual(order, ["Infra"]);
});
