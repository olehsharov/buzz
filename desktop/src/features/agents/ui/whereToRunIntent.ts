import type { BackendIntent } from "../lib/instanceInputForDefinition";
import type { BackendProviderProbeResult } from "@/shared/api/types";
import { coerceConfigValues } from "./ProviderConfigFields";

/** Draft state of the optional remote-backend selector. */
export type WhereToRunDraft = {
  runOn: "local" | string;
  providerConfig: Record<string, string>;
  probedProvider: BackendProviderProbeResult | null;
  /** Folder on the selected machine; blank = the machine's default. */
  hostWorkdir?: string;
};

export const emptyWhereToRunDraft: WhereToRunDraft = {
  runOn: "local",
  providerConfig: {},
  probedProvider: null,
};

/**
 * Fold a completed probe into the draft the user has *now* — not the draft
 * that existed when the probe started. Schema defaults prefill only the keys
 * the user has not touched: anything already in `providerConfig` (typed while
 * the probe was in flight) wins over the default. Overwriting instead of
 * merging is the "Typewriter Eraser" bug — every probe resolution silently
 * erased in-flight keystrokes. The same precedence keeps values an agent
 * draft prefilled (e.g. a working directory) over schema defaults.
 *
 * When the schema declares its properties, keys it does not declare are
 * dropped: the dialog renders only declared fields, so an undeclared
 * prefilled key would otherwise reach the provider without the owner ever
 * seeing it.
 */
export function applyProbeResult(
  current: WhereToRunDraft,
  result: BackendProviderProbeResult,
): WhereToRunDraft {
  const defaults: Record<string, string> = {};
  const declared = (result.config_schema as Record<string, unknown> | undefined)
    ?.properties as Record<string, Record<string, unknown>> | undefined;
  for (const [key, property] of Object.entries(declared ?? {})) {
    if (property.default != null) defaults[key] = String(property.default);
  }
  const visibleConfig = declared
    ? Object.fromEntries(
        Object.entries(current.providerConfig).filter(([key]) =>
          Object.hasOwn(declared, key),
        ),
      )
    : current.providerConfig;
  return {
    ...current,
    probedProvider: result,
    providerConfig: { ...defaults, ...visibleConfig },
  };
}

/** `runOn` values for approved machines are `host:<pubkey>`. */
export const HOST_RUN_ON_PREFIX = "host:";

export function hostRunOnValue(hostPubkey: string): string {
  return `${HOST_RUN_ON_PREFIX}${hostPubkey}`;
}

/** The machine pubkey a `runOn` value targets, or null for local/provider. */
export function hostPubkeyFromRunOn(runOn: string): string | null {
  return runOn.startsWith(HOST_RUN_ON_PREFIX)
    ? runOn.slice(HOST_RUN_ON_PREFIX.length) || null
    : null;
}

export function providerConfigComplete(draft: WhereToRunDraft): boolean {
  if (draft.runOn === "local") return true;
  if (hostPubkeyFromRunOn(draft.runOn)) return true;
  if (!draft.probedProvider) return false;
  const schema = draft.probedProvider.config_schema as
    | Record<string, unknown>
    | undefined;
  const required: string[] = (schema?.required as string[] | undefined) ?? [];
  return required.every(
    (key) => (draft.providerConfig[key] ?? "").trim().length > 0,
  );
}

export function canSubmitWhereToRun(draft: WhereToRunDraft): boolean {
  return providerConfigComplete(draft);
}

export function resolveBackendIntent(
  draft: WhereToRunDraft,
): BackendIntent | null {
  if (draft.runOn === "local") return null;
  const hostPubkey = hostPubkeyFromRunOn(draft.runOn);
  if (hostPubkey) {
    const workdir = draft.hostWorkdir?.trim();
    return workdir
      ? { type: "host", hostPubkey, workdir }
      : { type: "host", hostPubkey };
  }
  return {
    type: "provider",
    id: draft.runOn,
    config: coerceConfigValues(
      draft.providerConfig,
      draft.probedProvider?.config_schema,
    ),
  };
}
