import { Info } from "lucide-react";

/**
 * Notes about an agent-drafted create request that the owner must see before
 * approving: choices the dialog adjusted, or ones that live (and may block
 * Create) inside the collapsed Advanced section.
 */
export function AgentDraftReviewNotices({
  notices,
}: {
  notices: readonly string[];
}) {
  if (notices.length === 0) return null;
  return (
    <div
      className="flex gap-3 rounded-2xl border border-warning/30 bg-warning-bg px-4 py-3"
      data-testid="agent-draft-review-notices"
    >
      <Info
        aria-hidden="true"
        className="mt-0.5 h-4 w-4 shrink-0 text-warning"
      />
      <ul
        aria-label="About this draft"
        className="space-y-1 text-sm text-warning"
      >
        {notices.map((notice) => (
          <li key={notice}>{notice}</li>
        ))}
      </ul>
    </div>
  );
}
