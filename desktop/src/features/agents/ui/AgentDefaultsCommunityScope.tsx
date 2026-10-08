import { useOptionalCommunities } from "@/features/communities/useCommunities";

/**
 * Names the community whose agent defaults are being edited. Defaults are
 * per community (the backend reads and writes the invoking window's
 * community), so every defaults surface says which one it is: the active
 * community in the main window, the window's own in a community window.
 */
export function AgentDefaultsCommunityScope({
  lead,
}: {
  /** Sentence fragment before the community name. */
  lead: string;
}) {
  const communityName =
    useOptionalCommunities()?.activeCommunity?.name?.trim() || null;
  return (
    <>
      {lead}{" "}
      {communityName ? (
        <>
          agents in{" "}
          <span
            className="font-medium text-foreground"
            data-testid="agent-defaults-community-name"
          >
            {communityName}
          </span>
          .
        </>
      ) : (
        "agents in this community."
      )}{" "}
      Each community has its own defaults. Agent-specific settings always take
      priority.
    </>
  );
}
