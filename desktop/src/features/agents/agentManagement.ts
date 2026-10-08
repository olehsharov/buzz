import { emojiAvatarDataUrl } from "@/features/profile/ui/ProfileAvatarEditor.utils";
import type {
  AgentPersona,
  CreatePersonaInput,
  RespondToMode,
  UpdatePersonaInput,
} from "@/shared/api/types";
import type { WhereToRunDraft } from "./ui/whereToRunIntent";

export const AGENT_MANAGEMENT_REQUEST = "agent_management_request" as const;

/**
 * Respond-to modes an agent may request in a draft. `nobody` is a harness mode
 * Desktop definitions cannot store; the review dialog downgrades it to
 * owner-only and tells the owner (see {@link createRequestNotices}).
 */
export type DraftRespondTo = RespondToMode | "nobody";

/** Fields of an agent-drafted create request; the owner approves each draft. */
export type AgentDraftCreateFields = {
  channelId: string;
  displayName: string;
  /** May be empty: the owner writes the prompt in the review dialog. */
  systemPrompt: string;
  runtime?: string;
  model?: string;
  respondTo?: DraftRespondTo;
  envVars?: Record<string, string>;
  avatar?: { emoji: string; color: string };
  runOn?: string;
  providerConfig?: Record<string, string>;
};

export type AgentManagementCreateRequest = {
  type: typeof AGENT_MANAGEMENT_REQUEST;
  action: "create";
  requestId: string;
  request: AgentDraftCreateFields;
};

export type AgentManagementUpdateRequest = {
  type: typeof AGENT_MANAGEMENT_REQUEST;
  action: "update";
  requestId: string;
  request: {
    channelId: string;
    agentName: string;
    displayName?: string;
    systemPrompt?: string;
    runtime?: string;
    provider?: string;
    model?: string;
    respondTo?: RespondToMode;
  };
};

export type AgentManagementRequest =
  | AgentManagementCreateRequest
  | AgentManagementUpdateRequest;

function isText(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function isRespondTo(value: unknown): value is RespondToMode | undefined {
  return value === undefined || value === "owner-only" || value === "anyone";
}

function hasOnlyKeys(
  value: Record<string, unknown>,
  allowed: readonly string[],
) {
  return Object.keys(value).every((key) => allowed.includes(key));
}

const CREATE_REQUEST_KEYS = [
  "channelId",
  "displayName",
  "systemPrompt",
  "runtime",
  "model",
  "respondTo",
  "envVars",
  "avatar",
  "runOn",
  "providerConfig",
] as const;

/** Env keys an agent may draft; any other key rejects the whole request. */
export const DRAFT_ENV_VAR_KEYS: ReadonlySet<string> = new Set([
  "ANTHROPIC_AUTH_TOKEN",
  "ANTHROPIC_BASE_URL",
  "BUZZ_ACP_RESUME_SESSION",
]);

const DRAFT_ID_PATTERN = /^[a-z0-9][a-z0-9_-]*$/u;
const PROVIDER_CONFIG_KEY_PATTERN = /^[a-z_][a-z0-9_]*$/u;
const LOWERCASE_UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
const HEX_COLOR_PATTERN = /^#[0-9a-fA-F]{6}$/u;
const MAX_DISPLAY_NAME_CHARS = 120;
const MAX_ID_CHARS = 64;
const MAX_VALUE_CHARS = 300;
const MAX_PROVIDER_CONFIG_ENTRIES = 20;
const MAX_EMOJI_BYTES = 16;

/** Length in Unicode code points, matching Rust's `str::chars().count()`. */
function charCount(value: string) {
  return [...value].length;
}

function isBoundedString(value: unknown, max: number): value is string {
  return typeof value === "string" && charCount(value) <= max;
}

function isDraftId(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length <= MAX_ID_CHARS &&
    DRAFT_ID_PATTERN.test(value)
  );
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    Object.getPrototypeOf(value) === Object.prototype
  );
}

function isDraftRespondTo(value: unknown): value is DraftRespondTo {
  return (
    value === "owner-only" ||
    value === "allowlist" ||
    value === "anyone" ||
    value === "nobody"
  );
}

function parseDraftEnvVars(value: unknown): Record<string, string> | null {
  if (!isPlainRecord(value)) return null;
  const envVars: Record<string, string> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (
      !DRAFT_ENV_VAR_KEYS.has(key) ||
      !isBoundedString(entry, MAX_VALUE_CHARS)
    ) {
      return null;
    }
    if (
      key === "BUZZ_ACP_RESUME_SESSION" &&
      !LOWERCASE_UUID_PATTERN.test(entry)
    ) {
      return null;
    }
    envVars[key] = entry;
  }
  return envVars;
}

function isSingleGrapheme(value: string) {
  const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
  return [...segmenter.segment(value)].length === 1;
}

function parseDraftAvatar(
  value: unknown,
): { emoji: string; color: string } | null {
  if (!isPlainRecord(value) || !hasOnlyKeys(value, ["emoji", "color"])) {
    return null;
  }
  const { emoji, color } = value;
  if (
    typeof emoji !== "string" ||
    emoji.trim().length === 0 ||
    new TextEncoder().encode(emoji).length > MAX_EMOJI_BYTES ||
    !isSingleGrapheme(emoji) ||
    typeof color !== "string" ||
    !HEX_COLOR_PATTERN.test(color)
  ) {
    return null;
  }
  return { emoji, color };
}

function parseProviderConfig(value: unknown): Record<string, string> | null {
  if (!isPlainRecord(value)) return null;
  const entries = Object.entries(value);
  if (entries.length > MAX_PROVIDER_CONFIG_ENTRIES) return null;
  const config: Record<string, string> = {};
  for (const [key, entry] of entries) {
    if (
      !PROVIDER_CONFIG_KEY_PATTERN.test(key) ||
      !isBoundedString(entry, MAX_VALUE_CHARS)
    ) {
      return null;
    }
    config[key] = entry;
  }
  return config;
}

/**
 * Validate the create arm of the shared agent-draft contract. Every present
 * optional field must be valid and any unknown key rejects the request, so a
 * malformed or secret-shaped draft never reaches the owner's review dialog.
 */
function parseCreateFields(
  request: Record<string, unknown>,
): AgentDraftCreateFields | null {
  if (!hasOnlyKeys(request, CREATE_REQUEST_KEYS)) return null;
  const {
    channelId,
    displayName,
    systemPrompt,
    runtime,
    model,
    respondTo,
    runOn,
  } = request;
  if (
    !isText(channelId) ||
    !isText(displayName) ||
    charCount(displayName) > MAX_DISPLAY_NAME_CHARS ||
    typeof systemPrompt !== "string" ||
    (runtime !== undefined && !isDraftId(runtime)) ||
    (model !== undefined &&
      (!isText(model) || !isBoundedString(model, MAX_VALUE_CHARS))) ||
    (respondTo !== undefined && !isDraftRespondTo(respondTo)) ||
    (runOn !== undefined && !isDraftId(runOn)) ||
    (request.providerConfig !== undefined && runOn === undefined)
  ) {
    return null;
  }
  const envVars =
    request.envVars === undefined
      ? undefined
      : parseDraftEnvVars(request.envVars);
  const avatar =
    request.avatar === undefined ? undefined : parseDraftAvatar(request.avatar);
  const providerConfig =
    request.providerConfig === undefined
      ? undefined
      : parseProviderConfig(request.providerConfig);
  if (envVars === null || avatar === null || providerConfig === null) {
    return null;
  }
  return {
    channelId,
    displayName,
    systemPrompt,
    ...(runtime !== undefined ? { runtime } : {}),
    ...(model !== undefined ? { model } : {}),
    ...(respondTo !== undefined ? { respondTo } : {}),
    ...(envVars !== undefined ? { envVars } : {}),
    ...(avatar !== undefined ? { avatar } : {}),
    ...(runOn !== undefined ? { runOn } : {}),
    ...(providerConfig !== undefined ? { providerConfig } : {}),
  };
}

/** Parses the narrow agent-management request contract; unknown keys reject. */
export function parseAgentManagementRequest(
  value: unknown,
): AgentManagementRequest | null {
  if (typeof value !== "object" || value === null) return null;
  const payload = value as Record<string, unknown>;
  if (
    payload.type !== AGENT_MANAGEMENT_REQUEST ||
    !isText(payload.requestId) ||
    (payload.action !== "create" && payload.action !== "update") ||
    typeof payload.request !== "object" ||
    payload.request === null
  ) {
    return null;
  }
  const request = payload.request as Record<string, unknown>;

  if (payload.action === "create") {
    const fields = parseCreateFields(request);
    if (!fields) return null;
    return {
      type: AGENT_MANAGEMENT_REQUEST,
      action: "create",
      requestId: payload.requestId,
      request: fields,
    };
  }

  if (
    !isRespondTo(request.respondTo) ||
    !hasOnlyKeys(request, [
      "channelId",
      "agentName",
      "displayName",
      "systemPrompt",
      "runtime",
      "provider",
      "model",
      "respondTo",
    ]) ||
    !isText(request.channelId) ||
    !isText(request.agentName)
  ) {
    return null;
  }
  const changes = {
    ...(isText(request.displayName)
      ? { displayName: request.displayName }
      : {}),
    ...(isText(request.systemPrompt)
      ? { systemPrompt: request.systemPrompt }
      : {}),
    ...(isText(request.runtime) ? { runtime: request.runtime } : {}),
    ...(isText(request.provider) ? { provider: request.provider } : {}),
    ...(isText(request.model) ? { model: request.model } : {}),
    ...(request.respondTo ? { respondTo: request.respondTo } : {}),
  };
  if (Object.keys(changes).length === 0) return null;
  return {
    type: AGENT_MANAGEMENT_REQUEST,
    action: "update",
    requestId: payload.requestId,
    request: {
      channelId: request.channelId,
      agentName: request.agentName,
      ...changes,
    },
  };
}

export function requestTargetsEditablePersona(
  persona: AgentPersona | undefined,
): persona is AgentPersona {
  return Boolean(persona && !persona.sourceTeam);
}

/**
 * Seed the owner's review dialog from an agent draft. The avatar is always
 * rebuilt from the emoji+color pair with the shared emoji-avatar builder, so
 * a draft can never point the new agent at an arbitrary image URL.
 */
export function createInputFromRequest(
  request: Extract<AgentManagementRequest, { action: "create" }>,
): CreatePersonaInput {
  const fields = request.request;
  return {
    displayName: fields.displayName,
    systemPrompt: fields.systemPrompt,
    ...(fields.avatar
      ? {
          avatarUrl: emojiAvatarDataUrl(
            fields.avatar.emoji,
            fields.avatar.color,
          ),
        }
      : {}),
    ...(fields.runtime !== undefined ? { runtime: fields.runtime } : {}),
    ...(fields.model !== undefined ? { model: fields.model } : {}),
    ...(fields.envVars !== undefined ? { envVars: { ...fields.envVars } } : {}),
    ...(fields.respondTo !== undefined
      ? {
          behavior: {
            // Desktop cannot store `nobody`; owner-only is the most
            // restrictive mode it has, and the dialog says so.
            respondTo:
              fields.respondTo === "nobody" ? "owner-only" : fields.respondTo,
            respondToAllowlist: [],
          },
        }
      : {}),
  };
}

/**
 * Initial "Run on" draft for an agent's create request. A `runOn` that is not
 * a provider discovered on this computer falls back to local with a notice,
 * so the owner never faces a silently unsubmittable or mis-targeted form.
 */
export function runDraftFromRequest(
  fields: Pick<AgentDraftCreateFields, "runOn" | "providerConfig">,
  discoveredProviderIds: readonly string[],
): { draft: WhereToRunDraft; notice: string | null } {
  const local: WhereToRunDraft = {
    runOn: "local",
    providerConfig: {},
    probedProvider: null,
  };
  if (fields.runOn === undefined) return { draft: local, notice: null };
  if (!discoveredProviderIds.includes(fields.runOn)) {
    return {
      draft: local,
      notice: `This draft asks to run on “${fields.runOn}”, which is not set up on this computer. The agent will run on this computer instead.`,
    };
  }
  return {
    draft: {
      runOn: fields.runOn,
      providerConfig: { ...(fields.providerConfig ?? {}) },
      probedProvider: null,
    },
    notice: `This draft asks to run on “${fields.runOn}”. Review Run on under Advanced: that provider will receive the agent's private key.`,
  };
}

/**
 * Owner-facing notes for draft choices the dialog had to adjust, or that
 * would otherwise block Create from inside the collapsed Advanced section.
 * `catalogRuntimeIds` is null while the runtime catalog is still loading.
 */
export function createRequestNotices(
  fields: Pick<AgentDraftCreateFields, "respondTo" | "runtime">,
  catalogRuntimeIds: readonly string[] | null,
): string[] {
  const notices: string[] = [];
  if (
    fields.runtime !== undefined &&
    catalogRuntimeIds !== null &&
    !catalogRuntimeIds.includes(fields.runtime)
  ) {
    notices.push(
      `This draft asks for the “${fields.runtime}” harness, which is not available on this computer. Choose another harness.`,
    );
  }
  if (fields.respondTo === "nobody") {
    notices.push(
      "This draft asks for an agent that responds to nobody. Desktop agents always respond to you, so Respond to is set to Owner only.",
    );
  } else if (fields.respondTo === "allowlist") {
    notices.push(
      "This draft asks for an allowlist. Add at least one person under Advanced → Respond to before creating.",
    );
  }
  return notices;
}

/** Overlay an approved agent-requested edit without resetting unrequested behavior. */
export function updateInputFromRequest(
  request: Extract<AgentManagementRequest, { action: "update" }>,
  current: UpdatePersonaInput,
): UpdatePersonaInput {
  const changes = request.request;
  return {
    ...current,
    displayName: changes.displayName ?? current.displayName,
    systemPrompt: changes.systemPrompt ?? current.systemPrompt,
    runtime: changes.runtime ?? current.runtime,
    provider: changes.provider ?? current.provider,
    model: changes.model ?? current.model,
    ...(changes.respondTo
      ? {
          behavior: {
            respondTo: changes.respondTo,
            respondToAllowlist: [],
            parallelism: current.behavior?.parallelism,
            sessionPolicy: current.behavior?.sessionPolicy,
          },
        }
      : {}),
  };
}
