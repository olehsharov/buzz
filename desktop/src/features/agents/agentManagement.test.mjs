import assert from "node:assert/strict";
import test from "node:test";

import {
  AGENT_MANAGEMENT_REQUEST,
  createInputFromRequest,
  createRequestNotices,
  requestTargetsEditablePersona,
  parseAgentManagementRequest,
  runDraftFromRequest,
  updateInputFromRequest,
} from "./agentManagement.ts";
import {
  emojiAvatarDataUrl,
  parseEmojiAvatarDataUrl,
} from "../profile/ui/ProfileAvatarEditor.utils.ts";
import {
  behaviorForSubmit,
  draftFromBehavior,
} from "./ui/personaBehaviorDraft.ts";

const CHANNEL_ID = "7c07e659-3610-42f4-9a5e-1e9973c09da9";

function createPayload(overrides = {}) {
  return {
    type: AGENT_MANAGEMENT_REQUEST,
    action: "create",
    requestId: "request-1",
    request: {
      channelId: CHANNEL_ID,
      displayName: "Research helper",
      systemPrompt: "Find reliable sources and summarize them.",
    },
    ...overrides,
  };
}

test("parses the narrow no-secret create request", () => {
  assert.deepEqual(
    parseAgentManagementRequest(createPayload()),
    createPayload(),
  );
});

test("rejects an agent-management request with extra secret-shaped fields", () => {
  const payload = createPayload();
  payload.request.apiKey = "should-not-be-accepted";

  assert.equal(parseAgentManagementRequest(payload), null);
});

// Product decision: an agent's create draft may now propose the runtime,
// model, access, credentials-env, avatar, and run location, because the owner
// still approves every draft with a click in the review dialog. The draft
// contract stays closed — each field is validated and any other key (for
// example `provider` or `apiKey`) still rejects the whole request.

const RESUME_SESSION = "35477ccb-3acc-499e-8b62-028f5b194ffd";

function withField(field, value) {
  const payload = createPayload();
  payload.request[field] = value;
  return payload;
}

const VALID_FIELDS = [
  ["runtime", "claude"],
  ["runtime", "buzz-agent_2"],
  ["runtime", "a".repeat(64)],
  ["model", "claude-opus-4"],
  ["model", "m".repeat(300)],
  ["respondTo", "owner-only"],
  ["respondTo", "allowlist"],
  ["respondTo", "anyone"],
  ["respondTo", "nobody"],
  ["envVars", {}],
  ["envVars", { ANTHROPIC_AUTH_TOKEN: "" }],
  [
    "envVars",
    {
      ANTHROPIC_AUTH_TOKEN: "token",
      ANTHROPIC_BASE_URL: "http://localhost:4001",
      BUZZ_ACP_RESUME_SESSION: RESUME_SESSION,
    },
  ],
  ["envVars", { ANTHROPIC_BASE_URL: "u".repeat(300) }],
  ["avatar", { emoji: "🦊", color: "#FFB84D" }],
  ["avatar", { emoji: "👩‍💻", color: "#ffb84d" }],
  ["displayName", "n".repeat(120)],
  ["displayName", "🦊".repeat(120)],
  ["systemPrompt", ""],
  ["runOn", "blox"],
];

for (const [field, value] of VALID_FIELDS) {
  test(`create draft accepts ${field}=${JSON.stringify(value).slice(0, 60)}`, () => {
    const payload = withField(field, value);
    assert.deepEqual(parseAgentManagementRequest(payload), payload);
  });
}

test("create draft accepts providerConfig together with runOn", () => {
  const payload = createPayload();
  payload.request.runOn = "blox";
  payload.request.providerConfig = { workdir: "/srv/agents", _region: "us" };
  assert.deepEqual(parseAgentManagementRequest(payload), payload);
});

const INVALID_FIELDS = [
  ["provider", "anthropic"],
  ["apiKey", "secret"],
  ["avatarUrl", "https://example.com/a.png"],
  ["channelId", ""],
  ["displayName", "   "],
  ["displayName", "n".repeat(121)],
  ["systemPrompt", null],
  ["systemPrompt", 3],
  ["runtime", ""],
  ["runtime", "Claude"],
  ["runtime", "-claude"],
  ["runtime", "claude code"],
  ["runtime", "a".repeat(65)],
  ["runtime", null],
  ["model", ""],
  ["model", "   "],
  ["model", "m".repeat(301)],
  ["model", 4],
  ["respondTo", "owner_only"],
  ["respondTo", "everyone"],
  ["respondTo", null],
  ["envVars", { OPENAI_API_KEY: "secret" }],
  ["envVars", { ANTHROPIC_AUTH_TOKEN: "ok", PATH: "/tmp" }],
  ["envVars", { ANTHROPIC_BASE_URL: "u".repeat(301) }],
  ["envVars", { ANTHROPIC_BASE_URL: 1 }],
  ["envVars", { BUZZ_ACP_RESUME_SESSION: RESUME_SESSION.toUpperCase() }],
  ["envVars", { BUZZ_ACP_RESUME_SESSION: "not-a-uuid" }],
  ["envVars", { BUZZ_ACP_RESUME_SESSION: "" }],
  ["envVars", ["ANTHROPIC_BASE_URL"]],
  ["envVars", null],
  ["avatar", { emoji: "🦊" }],
  ["avatar", { color: "#FFB84D" }],
  ["avatar", { emoji: "🦊", color: "#FFB84D", url: "https://x" }],
  ["avatar", { emoji: "🦊🦊", color: "#FFB84D" }],
  ["avatar", { emoji: "ab", color: "#FFB84D" }],
  ["avatar", { emoji: " ", color: "#FFB84D" }],
  ["avatar", { emoji: "", color: "#FFB84D" }],
  ["avatar", { emoji: "👨‍👩‍👧‍👦", color: "#FFB84D" }],
  ["avatar", { emoji: "🦊", color: "#FFB84" }],
  ["avatar", { emoji: "🦊", color: "orange" }],
  ["avatar", { emoji: "🦊", color: "#GGGGGG" }],
  ["avatar", "🦊"],
  ["runOn", "Blox"],
  ["runOn", ""],
  ["runOn", "r".repeat(65)],
  ["providerConfig", { workdir: "/srv" }],
];

for (const [field, value] of INVALID_FIELDS) {
  test(`create draft rejects ${field}=${JSON.stringify(value)?.slice(0, 60)}`, () => {
    assert.equal(parseAgentManagementRequest(withField(field, value)), null);
  });
}

test("create draft rejects malformed providerConfig alongside runOn", () => {
  for (const providerConfig of [
    { Workdir: "/srv" },
    { "9lives": "x" },
    { workdir: 5 },
    { workdir: "w".repeat(301) },
    Object.fromEntries(
      Array.from({ length: 21 }, (_, index) => [`k${index}`, "v"]),
    ),
    ["workdir"],
  ]) {
    const payload = createPayload();
    payload.request.runOn = "blox";
    payload.request.providerConfig = providerConfig;
    assert.equal(
      parseAgentManagementRequest(payload),
      null,
      JSON.stringify(providerConfig),
    );
  }
});

test("create draft requires displayName, systemPrompt, and channelId keys", () => {
  for (const field of ["channelId", "displayName", "systemPrompt"]) {
    const payload = createPayload();
    delete payload.request[field];
    assert.equal(parseAgentManagementRequest(payload), null, field);
  }
});

test("chat creation leaves advanced behavior unset so the form stays collapsed", () => {
  const parsed = parseAgentManagementRequest(createPayload());
  assert.ok(parsed && parsed.action === "create");

  assert.deepEqual(createInputFromRequest(parsed), {
    displayName: "Research helper",
    systemPrompt: "Find reliable sources and summarize them.",
  });
});

function fullDraft() {
  const payload = createPayload();
  Object.assign(payload.request, {
    systemPrompt: "",
    runtime: "claude",
    model: "claude-opus-4",
    respondTo: "anyone",
    envVars: {
      ANTHROPIC_BASE_URL: "http://localhost:4001",
      BUZZ_ACP_RESUME_SESSION: RESUME_SESSION,
    },
    avatar: { emoji: "🦊", color: "#FFB84D" },
    runOn: "blox",
    providerConfig: { workdir: "/srv/agents" },
  });
  return parseAgentManagementRequest(payload);
}

test("a full draft seeds the review dialog with every proposed field", () => {
  const parsed = fullDraft();
  assert.ok(parsed && parsed.action === "create");

  assert.deepEqual(createInputFromRequest(parsed), {
    displayName: "Research helper",
    systemPrompt: "",
    avatarUrl: emojiAvatarDataUrl("🦊", "#FFB84D"),
    runtime: "claude",
    model: "claude-opus-4",
    envVars: {
      ANTHROPIC_BASE_URL: "http://localhost:4001",
      BUZZ_ACP_RESUME_SESSION: RESUME_SESSION,
    },
    behavior: { respondTo: "anyone", respondToAllowlist: [] },
  });
});

test("a draft avatar is the shared emoji avatar, never a URL from the draft", () => {
  const parsed = fullDraft();
  const { avatarUrl } = createInputFromRequest(parsed);
  assert.deepEqual(parseEmojiAvatarDataUrl(avatarUrl), {
    emoji: "🦊",
    color: "#FFB84D",
  });
});

test("a draft asking for nobody is seeded as owner-only and explained", () => {
  const parsed = parseAgentManagementRequest(withField("respondTo", "nobody"));
  assert.deepEqual(createInputFromRequest(parsed).behavior, {
    respondTo: "owner-only",
    respondToAllowlist: [],
  });
  assert.equal(createRequestNotices(parsed.request, null).length, 1);
  assert.match(createRequestNotices(parsed.request, null)[0], /Owner only/);
});

test("a draft asking for an allowlist tells the owner to add people first", () => {
  const parsed = parseAgentManagementRequest(
    withField("respondTo", "allowlist"),
  );
  assert.deepEqual(createInputFromRequest(parsed).behavior, {
    respondTo: "allowlist",
    respondToAllowlist: [],
  });
  const [notice] = createRequestNotices(parsed.request, null);
  assert.match(notice, /Add at least one person/);
});

test("a draft runtime missing from the catalog is called out once the catalog loads", () => {
  const fields = { runtime: "claude" };
  assert.deepEqual(createRequestNotices(fields, null), []);
  assert.deepEqual(createRequestNotices(fields, ["claude", "goose"]), []);
  const [notice] = createRequestNotices(fields, ["goose"]);
  assert.match(notice, /“claude” harness/);
});

test("a draft without access or runtime choices needs no notices", () => {
  assert.deepEqual(createRequestNotices(createPayload().request, []), []);
});

test("runDraftFromRequest starts local when the draft names no provider", () => {
  assert.deepEqual(runDraftFromRequest({}, ["blox"]), {
    draft: { runOn: "local", providerConfig: {}, probedProvider: null },
    notice: null,
  });
});

test("runDraftFromRequest selects a discovered provider with its config", () => {
  const { draft, notice } = runDraftFromRequest(
    { runOn: "blox", providerConfig: { workdir: "/srv/agents" } },
    ["kubernetes", "blox"],
  );
  assert.deepEqual(draft, {
    runOn: "blox",
    providerConfig: { workdir: "/srv/agents" },
    probedProvider: null,
  });
  // The run section sits under collapsed Advanced, so the owner is told up
  // front that a remote provider (which receives the private key) is chosen.
  assert.match(notice, /“blox”/);
  assert.match(notice, /private key/);
});

test("runDraftFromRequest falls back to local, visibly, for an unknown provider", () => {
  const { draft, notice } = runDraftFromRequest(
    { runOn: "blox", providerConfig: { workdir: "/srv/agents" } },
    ["kubernetes"],
  );
  assert.deepEqual(draft, {
    runOn: "local",
    providerConfig: {},
    probedProvider: null,
  });
  assert.match(notice, /not set up on this computer/);
});

test("requires the originating channel for profile updates", () => {
  const payload = {
    type: AGENT_MANAGEMENT_REQUEST,
    action: "update",
    requestId: "request-2",
    request: {
      agentName: "Review helper",
      systemPrompt: "Review changes concisely.",
    },
  };

  assert.equal(parseAgentManagementRequest(payload), null);
});

test("uses an agent's current name, never an internal profile ID", () => {
  const payload = {
    type: AGENT_MANAGEMENT_REQUEST,
    action: "update",
    requestId: "request-3",
    request: {
      channelId: CHANNEL_ID,
      agentName: "Review helper",
      systemPrompt: "Review changes concisely.",
    },
  };

  assert.deepEqual(parseAgentManagementRequest(payload), payload);
});

test("allows agents to update only personal, editable profiles", () => {
  assert.equal(
    requestTargetsEditablePersona({ isBuiltIn: false, sourceTeam: null }),
    true,
  );
  assert.equal(
    requestTargetsEditablePersona({ isBuiltIn: true, sourceTeam: null }),
    true,
  );
  assert.equal(
    requestTargetsEditablePersona({ isBuiltIn: false, sourceTeam: "team" }),
    false,
  );
});

test("agent-requested access edits preserve thread-scoped conversation context", () => {
  const request = {
    type: AGENT_MANAGEMENT_REQUEST,
    action: "update",
    requestId: "request-4",
    request: {
      channelId: CHANNEL_ID,
      agentName: "Review helper",
      respondTo: "anyone",
    },
  };
  const updated = updateInputFromRequest(request, {
    id: "review-helper",
    displayName: "Review helper",
    systemPrompt: "Review changes concisely.",
    behavior: {
      respondTo: "owner-only",
      respondToAllowlist: [],
      parallelism: 2,
      sessionPolicy: "thread",
    },
  });

  assert.equal(updated.behavior?.sessionPolicy, "thread");

  const seed = draftFromBehavior(updated.behavior);
  const edited = { ...seed, parallelism: "3" };
  assert.deepEqual(behaviorForSubmit(edited, seed, true), {
    respondTo: "anyone",
    respondToAllowlist: undefined,
    parallelism: 3,
    sessionPolicy: "thread",
  });
});
