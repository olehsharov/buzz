import { invokeTauri } from "@/shared/api/tauri";
import type {
  GlobalAgentConfig,
  GlobalAgentConfigSaveResult,
} from "@/shared/api/types";

/**
 * Read the agent defaults of this window's community (the active community,
 * or a community window's own). Defaults are per community: an agent only
 * ever receives its own community's.
 *
 * Returns an empty default if none were saved for this community yet.
 */
export async function getGlobalAgentConfig(): Promise<GlobalAgentConfig> {
  return invokeTauri<GlobalAgentConfig>("get_global_agent_config");
}

/**
 * Validate and persist the agent defaults of this window's community.
 *
 * The backend strips empty env values (empty = "inherit"), validates key
 * shape and reserved-key rules, restarts this community's running local
 * agents whose effective env changed, and returns the saved config with a
 * restart count. Other communities' defaults are untouched.
 *
 * Throws a string error message on validation failure.
 */
export async function setGlobalAgentConfig(
  config: GlobalAgentConfig,
): Promise<GlobalAgentConfigSaveResult> {
  return invokeTauri<GlobalAgentConfigSaveResult>("set_global_agent_config", {
    config,
  });
}
