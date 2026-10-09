import assert from "node:assert/strict";
import test from "node:test";
import { npubEncode } from "nostr-tools/nip19";

import {
  AGENT_MANAGEMENT_REQUEST,
  createInputFromRequest,
  parseAgentManagementRequest,
  runDraftFromRequest,
} from "./agentManagement.ts";
import {
  canSubmitWhereToRun,
  resolveBackendIntent,
} from "./ui/whereToRunIntent.ts";

const CHANNEL_ID = "7c07e659-3610-42f4-9a5e-1e9973c09da9";
const RESUME = "0f3c2a9e-6b1d-4e7a-9c55-2d8f1a3b4c6e";
const DEVBOX = { pubkey: "a1".repeat(32), name: "Devbox" };
const BUILDER = { pubkey: "b2".repeat(32), name: "builder" };
const HOSTS = [DEVBOX, BUILDER];

function createPayload(request) {
  return {
    type: AGENT_MANAGEMENT_REQUEST,
    action: "create",
    requestId: "request-host",
    request: {
      channelId: CHANNEL_ID,
      displayName: "Resumed session",
      systemPrompt: "",
      ...request,
    },
  };
}

test("a machine draft parses with its folder", () => {
  const parsed = parseAgentManagementRequest(
    createPayload({ runOnHost: " devbox ", hostWorkdir: " ~/code/app " }),
  );
  assert.equal(parsed.request.runOnHost, "devbox");
  assert.equal(parsed.request.hostWorkdir, "~/code/app");
});

test("a malformed machine draft is rejected whole", () => {
  const cases = {
    "machine with a provider": { runOnHost: "devbox", runOn: "blox" },
    "folder without a machine": { hostWorkdir: "/srv/app" },
    "folder with a provider": { runOn: "blox", hostWorkdir: "/srv/app" },
    "provider config with a machine": {
      runOnHost: "devbox",
      providerConfig: { workdir: "/srv/app" },
    },
    "blank machine": { runOnHost: "  " },
    "machine name too long": { runOnHost: "m".repeat(129) },
    "machine name with a control character": { runOnHost: "dev\u0007box" },
    "non-string machine": { runOnHost: 7 },
    "blank folder": { runOnHost: "devbox", hostWorkdir: " " },
    "folder too long": { runOnHost: "devbox", hostWorkdir: "a".repeat(301) },
    "folder with a newline": { runOnHost: "devbox", hostWorkdir: "/a\nb" },
    "folder with a carriage return": {
      runOnHost: "devbox",
      hostWorkdir: "/a\rb",
    },
    "folder with NUL": { runOnHost: "devbox", hostWorkdir: "/a\u0000b" },
  };
  for (const [name, request] of Object.entries(cases)) {
    assert.equal(
      parseAgentManagementRequest(createPayload(request)),
      null,
      name,
    );
  }
});

function expectMachine(draft, host, workdir) {
  assert.equal(draft.runOn, `host:${host.pubkey}`);
  assert.equal(draft.hostWorkdir, workdir);
  assert.equal(canSubmitWhereToRun(draft), true);
  assert.deepEqual(resolveBackendIntent(draft), {
    type: "host",
    hostPubkey: host.pubkey,
    workdir,
  });
}

test("a machine draft resolves by hex pubkey, npub, or name and prefills the folder", () => {
  for (const reference of [
    DEVBOX.pubkey.toUpperCase(),
    npubEncode(DEVBOX.pubkey),
    "DEVBOX",
    " devbox ",
  ]) {
    const { draft, notice } = runDraftFromRequest(
      { runOnHost: reference, hostWorkdir: "/srv/app" },
      [],
      HOSTS,
    );
    expectMachine(draft, DEVBOX, "/srv/app");
    assert.match(notice, /your machine “Devbox” in “\/srv\/app”/, reference);
  }
});

test("a machine draft without a folder uses the machine's default", () => {
  const { draft } = runDraftFromRequest({ runOnHost: "builder" }, [], HOSTS);
  assert.equal(draft.runOn, `host:${BUILDER.pubkey}`);
  assert.equal("hostWorkdir" in draft, false);
  assert.deepEqual(resolveBackendIntent(draft), {
    type: "host",
    hostPubkey: BUILDER.pubkey,
  });
});

function expectUnset(result, pattern) {
  assert.deepEqual(result.draft, {
    runOn: "",
    providerConfig: {},
    probedProvider: null,
  });
  // Unset Run on blocks Create until the owner picks a target.
  assert.equal(canSubmitWhereToRun(result.draft), false);
  assert.match(result.notice, pattern);
  assert.match(result.notice, /Choose Run on under Advanced/);
}

test("an ambiguous machine name leaves Run on unset and says why", () => {
  const twins = [...HOSTS, { pubkey: "c3".repeat(32), name: "DevBox" }];
  const result = runDraftFromRequest(
    { runOnHost: "devbox", hostWorkdir: "~/code/app" },
    [],
    twins,
  );
  expectUnset(result, /machine “devbox” in “~\/code\/app”, but 2 of your/);
});

test("an unknown machine leaves Run on unset and names what was asked", () => {
  for (const reference of ["laptop", "d4".repeat(32)]) {
    const result = runDraftFromRequest({ runOnHost: reference }, [], HOSTS);
    expectUnset(result, /not one of your approved machines/);
    assert.ok(result.notice.includes(`“${reference}”`), reference);
  }
});

test("machines that could not be loaded leave Run on unset", () => {
  const result = runDraftFromRequest({ runOnHost: "devbox" }, [], null);
  expectUnset(result, /could not load your machines/);
});

test("a provider draft ignores the machine list", () => {
  const { draft } = runDraftFromRequest(
    { runOn: "blox", providerConfig: { workdir: "/srv/agents" } },
    ["blox"],
    HOSTS,
  );
  assert.deepEqual(draft, {
    runOn: "blox",
    providerConfig: { workdir: "/srv/agents" },
    probedProvider: null,
  });
});

test("a resumed session and its folder are prefilled together", () => {
  const parsed = parseAgentManagementRequest(
    createPayload({
      runtime: "claude",
      envVars: { BUZZ_ACP_RESUME_SESSION: RESUME },
      runOnHost: "devbox",
      hostWorkdir: "/home/me/code/app",
    }),
  );
  assert.deepEqual(createInputFromRequest(parsed).envVars, {
    BUZZ_ACP_RESUME_SESSION: RESUME,
  });
  const { draft } = runDraftFromRequest(parsed.request, [], HOSTS);
  expectMachine(draft, DEVBOX, "/home/me/code/app");
});
