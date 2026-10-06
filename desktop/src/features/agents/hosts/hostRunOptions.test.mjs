import assert from "node:assert/strict";
import test from "node:test";

import {
  buildHostRunOnOptions,
  describeHost,
  describeHostClaude,
  excludeMachineAgents,
  formatHostLastSeen,
  hostAcceptsDeploys,
  hostAvailability,
  hostForAgent,
} from "./hostRunOptions.ts";
import {
  canSubmitWhereToRun,
  hostPubkeyFromRunOn,
  hostRunOnValue,
  resolveBackendIntent,
} from "../ui/whereToRunIntent.ts";

const A = "a".repeat(64);
const B = "b".repeat(64);
const NOW = 1_800_000_000_000;

function host(pubkey, name, extra = {}) {
  return {
    pubkey,
    name,
    os: "linux",
    arch: "x86_64",
    relayUrl: "wss://relay.example",
    addedAt: "",
    status: null,
    ...extra,
  };
}

test("availability: full table over presence state and load state", () => {
  const cases = [
    [{ [A]: "online" }, true, "online", true],
    [{ [A]: "away" }, true, "away", true],
    [{ [A]: "offline" }, true, "offline", false],
    [{}, true, "offline", false], // absent from a loaded lookup = offline
    [{ [A]: "online" }, false, "unknown", false], // not loaded yet
    [undefined, true, "unknown", false],
  ];
  for (const [lookup, loaded, expected, deployable] of cases) {
    const availability = hostAvailability(lookup, loaded, A.toUpperCase());
    assert.equal(availability, expected, JSON.stringify({ lookup, loaded }));
    assert.equal(hostAcceptsDeploys(availability), deployable);
  }
});

test("options list only the given approved hosts, sorted, offline disabled", () => {
  const options = buildHostRunOnOptions(
    [host(B, "zeta"), host(A, "alpha")],
    { [A]: "online", [B]: "offline" },
    true,
    NOW,
  );
  assert.deepEqual(
    options.map((option) => [option.label, option.value, option.disabled]),
    [
      ["alpha", hostRunOnValue(A), false],
      ["zeta", hostRunOnValue(B), true],
    ],
  );
  assert.equal(options[0].description, "Linux · x86_64 · Online");
  assert.equal(options[1].description, "Linux · x86_64 · Offline");
  assert.equal(options[1].presence, "offline");
});

test("offline description carries last seen from the newest status", () => {
  const seen = host(A, "alpha", {
    os: "macos",
    arch: "arm64",
    status: { receivedAt: NOW / 1000 - 300 },
  });
  assert.equal(
    describeHost(seen, "offline", NOW),
    "macOS · arm64 · Offline, last seen 5 min ago",
  );
  assert.equal(formatHostLastSeen(null, NOW), null);
  assert.equal(formatHostLastSeen(NOW / 1000 - 30, NOW), "last seen just now");
  assert.equal(formatHostLastSeen(NOW / 1000 - 7200, NOW), "last seen 2 h ago");
  assert.equal(describeHost(seen, "unknown", NOW), "macOS · arm64 · Checking…");
});

test("host runOn values round-trip into a host backend intent", () => {
  const draft = {
    runOn: hostRunOnValue(A),
    providerConfig: {},
    probedProvider: null,
  };
  assert.equal(hostPubkeyFromRunOn(draft.runOn), A);
  assert.equal(hostPubkeyFromRunOn("local"), null);
  assert.equal(hostPubkeyFromRunOn("kubernetes"), null);
  assert.equal(hostPubkeyFromRunOn("host:"), null);
  // A host needs no provider probe to be submittable.
  assert.equal(canSubmitWhereToRun(draft), true);
  assert.deepEqual(resolveBackendIntent(draft), {
    type: "host",
    hostPubkey: A,
  });
  // An unprobed provider is still blocked.
  assert.equal(
    canSubmitWhereToRun({
      runOn: "kubernetes",
      providerConfig: {},
      probedProvider: null,
    }),
    false,
  );
});

test("hostForAgent resolves only host backends", () => {
  const hosts = [host(A, "alpha")];
  assert.equal(
    hostForAgent(hosts, { type: "host", host_pubkey: A.toUpperCase() })?.name,
    "alpha",
  );
  assert.equal(hostForAgent(hosts, { type: "host", host_pubkey: B }), null);
  assert.equal(hostForAgent(hosts, { type: "local" }), null);
});

test("approved machines never show up as relay agents", () => {
  const agents = [{ pubkey: A.toUpperCase() }, { pubkey: B }];
  assert.deepEqual(excludeMachineAgents(agents, [host(A, "alpha")]), [
    { pubkey: B },
  ]);
  assert.deepEqual(excludeMachineAgents(agents, []), agents);
});

test("claude state: unknown sign-in is not shown as ready", () => {
  const status = (installed, authOk) => ({ claude: { installed, authOk } });
  assert.equal(describeHostClaude(null), "Claude: unknown");
  assert.equal(
    describeHostClaude(status(false, null)),
    "Claude: not installed",
  );
  assert.equal(
    describeHostClaude(status(true, null)),
    "Claude: installed, sign-in unknown",
  );
  assert.equal(describeHostClaude(status(true, true)), "Claude: ready");
  assert.equal(
    describeHostClaude(status(true, false)),
    "Claude: not signed in",
  );
});
