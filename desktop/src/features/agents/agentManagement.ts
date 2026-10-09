import { emojiAvatarDataUrl } from "@/features/profile/ui/ProfileAvatarEditor.utils";
import type {
  AgentPersona,
  CreatePersonaInput,
  RespondToMode,
  UpdatePersonaInput,
} from "@/shared/api/types";
import type { AgentHost } from "@/shared/api/agentHosts";
import { parsePubkeyInput } from "@/shared/lib/nostrUtils";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { hostRunOnValue, type WhereToRunDraft } from "./ui/whereToRunIntent";

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
  /** A compute provider id; exclusive with `runOnHost`. */
  runOn?: string;
  providerConfig?: Record<string, string>;
  /**
   * One of the owner's paired machines, as a hex pubkey, npub, or machine
   * name. Resolved against approved machines in {@link runDraftFromRequest}.
   */
  runOnHost?: string;
  /** Folder on that machine; only valid with `runOnHost`. */
  hostWorkdir?: string;
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
  "runOnHost",
  "hostWorkdir",
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
/** Longest machine name a machine's hello frame may carry. */
const MAX_HOST_REFERENCE_CHARS = 128;
/** Mirrors the backend's `MAX_HOST_WORKDIR_CHARS`. */
const MAX_HOST_WORKDIR_CHARS = 300;
// biome-ignore lint/suspicious/noControlCharactersInRegex: rejecting control characters is the point
const CONTROL_CHARACTER_PATTERN = /[\u0000-\u001f\u007f-\u009f]/u;
// biome-ignore lint/suspicious/noControlCharactersInRegex: NUL and line breaks are invalid in a folder path
const INVALID_WORKDIR_CHARACTER_PATTERN = /[\u0000\n\r]/u;

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

function isHostReference(value: unknown): value is string {
  return (
    isText(value) &&
    charCount(value) <= MAX_HOST_REFERENCE_CHARS &&
    !CONTROL_CHARACTER_PATTERN.test(value)
  );
}

function isHostWorkdir(value: unknown): value is string {
  return (
    isText(value) &&
    charCount(value) <= MAX_HOST_WORKDIR_CHARS &&
    !INVALID_WORKDIR_CHARACTER_PATTERN.test(value)
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
    runOnHost,
    hostWorkdir,
  } = request;
  // A machine target is exclusive with a provider, and a machine folder needs
  // a machine.
  const host =
    runOnHost === undefined
      ? undefined
      : isHostReference(runOnHost) && runOn === undefined
        ? runOnHost.trim()
        : null;
  const hostFolder =
    hostWorkdir === undefined
      ? undefined
      : isHostWorkdir(hostWorkdir) && host !== undefined
        ? hostWorkdir.trim()
        : null;
  if (host === null || hostFolder === null) return null;
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
    ...(host !== undefined ? { runOnHost: host } : {}),
    ...(hostFolder !== undefined ? { hostWorkdir: hostFolder } : {}),
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

type DraftHost = Pick<AgentHost, "pubkey" | "name">;

export type DraftHostResolution =
  | { status: "found"; host: DraftHost }
  | { status: "ambiguous"; count: number }
  | { status: "unknown" };

/**
 * Find the approved machine a draft names: a hex pubkey or npub matches the
 * machine's key; anything else matches machine names case-insensitively.
 * More than one machine with that name is ambiguous, never a guess.
 */
export function resolveDraftHost(
  reference: string,
  hosts: readonly DraftHost[],
): DraftHostResolution {
  const pubkey = parsePubkeyInput(reference);
  if (pubkey) {
    const host = hosts.find(
      (entry) => normalizePubkey(entry.pubkey) === pubkey,
    );
    return host ? { status: "found", host } : { status: "unknown" };
  }
  const name = reference.trim().toLowerCase();
  const matches = hosts.filter(
    (entry) => entry.name.trim().toLowerCase() === name,
  );
  if (matches.length === 1) return { status: "found", host: matches[0] };
  return matches.length > 1
    ? { status: "ambiguous", count: matches.length }
    : { status: "unknown" };
}

/**
 * Run on draft for a machine target. A machine that cannot be resolved
 * leaves Run on unset (Create stays blocked until the owner picks one) and
 * says what the draft asked for, so the request is never silently dropped
 * or sent somewhere else.
 */
function hostRunDraft(
  reference: string,
  workdir: string | undefined,
  approvedHosts: readonly DraftHost[] | null,
): { draft: WhereToRunDraft; notice: string } {
  const folder = workdir ? ` in “${workdir}”` : "";
  const resolution =
    approvedHosts === null ? null : resolveDraftHost(reference, approvedHosts);
  if (resolution?.status === "found") {
    return {
      draft: {
        runOn: hostRunOnValue(resolution.host.pubkey),
        providerConfig: {},
        probedProvider: null,
        ...(workdir ? { hostWorkdir: workdir } : {}),
      },
      notice: `This draft asks to run on your machine “${resolution.host.name}”${folder}. Review Run on under Advanced: Buzz sends the agent's key to that machine.`,
    };
  }
  const reason =
    resolution === null
      ? "Buzz could not load your machines"
      : resolution.status === "ambiguous"
        ? `${resolution.count} of your machines have that name`
        : "it is not one of your approved machines";
  return {
    // Unset: no option matches, so the owner must choose before Create.
    draft: { runOn: "", providerConfig: {}, probedProvider: null },
    notice: `This draft asks to run on the machine “${reference}”${folder}, but ${reason}. Choose Run on under Advanced before creating.`,
  };
}

/**
 * Initial "Run on" draft for an agent's create request. A `runOn` that is not
 * a provider discovered on this computer falls back to local with a notice,
 * so the owner never faces a silently unsubmittable or mis-targeted form.
 * A `runOnHost` resolves against `approvedHosts` (null when they could not
 * be loaded); see {@link hostRunDraft}.
 */
export function runDraftFromRequest(
  fields: Pick<
    AgentDraftCreateFields,
    "runOn" | "providerConfig" | "runOnHost" | "hostWorkdir"
  >,
  discoveredProviderIds: readonly string[],
  approvedHosts: readonly DraftHost[] | null,
): { draft: WhereToRunDraft; notice: string | null } {
  const local: WhereToRunDraft = {
    runOn: "local",
    providerConfig: {},
    probedProvider: null,
  };
  if (fields.runOnHost !== undefined) {
    return hostRunDraft(fields.runOnHost, fields.hostWorkdir, approvedHosts);
  }
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
