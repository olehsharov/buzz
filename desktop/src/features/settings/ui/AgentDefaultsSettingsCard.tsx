import { AgentDefaultsCommunityScope } from "@/features/agents/ui/AgentDefaultsCommunityScope";
import { AgentDefaultsEditor } from "@/features/agents/ui/AgentDefaultsEditor";
import { SettingsOptionGroup } from "./SettingsOptionGroup";

export function AgentDefaultsSettingsCard() {
  return (
    <SettingsOptionGroup
      data-testid="settings-global-agent-config"
      description={
        <AgentDefaultsCommunityScope lead="Provider, model, effort, and environment settings inherited by" />
      }
      title="Agent defaults"
    >
      <div className="px-4 py-4">
        <AgentDefaultsEditor layout="flat" />
      </div>
    </SettingsOptionGroup>
  );
}
