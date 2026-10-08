import * as React from "react";

import {
  closePartialMarkdown,
  type StreamDraft,
  selectUnrenderedStreamDrafts,
  timelineRowCompletionMessage,
} from "@/features/messages/lib/streamDrafts";
import type { TimelineMessage } from "@/features/messages/types";
import { MessageAuthorText } from "@/features/messages/ui/MessageHeader";
import {
  resolveUserLabel,
  type UserProfileLookup,
} from "@/features/profile/lib/identity";
import { cn } from "@/shared/lib/cn";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { Markdown } from "@/shared/ui/markdown";
import { Shimmer } from "@/shared/ui/Shimmer";
import { UserAvatar } from "@/shared/ui/UserAvatar";

/** How long a start/finish announcement stays in the live region. */
const ANNOUNCEMENT_CLEAR_MS = 4_000;

type StreamDraftRowsProps = {
  /** Ghosts already narrowed to this surface's scope. */
  drafts: readonly StreamDraft[];
  /** Rows this surface has actually rendered (for same-commit hand-off). */
  renderedMessages: readonly TimelineMessage[];
  currentPubkey?: string;
  profiles?: UserProfileLookup;
  className?: string;
};

function draftAuthorLabel(
  draft: StreamDraft,
  profiles: UserProfileLookup | undefined,
  currentPubkey: string | undefined,
) {
  return resolveUserLabel({
    pubkey: draft.pubkey,
    currentPubkey,
    profiles,
    preferResolvedSelfLabel: true,
  });
}

export function formatStreamDraftActivity(
  author: string,
  draft: Pick<StreamDraft, "label" | "status">,
) {
  if (draft.status === "tool") {
    return draft.label
      ? `${author} is running ${draft.label}…`
      : `${author} is using a tool…`;
  }
  return draft.status === "writing"
    ? `${author} is writing…`
    : `${author} is thinking…`;
}

/**
 * Polite start/finish announcements for ghosts. Never per chunk: content
 * updates are not announced; only a ghost appearing and its reply landing.
 * The text is cleared after a few seconds so the region does not linger as
 * an extra screen-reader stop.
 */
function useStreamDraftAnnouncement(
  allDrafts: readonly StreamDraft[],
  visibleDrafts: readonly StreamDraft[],
  labelFor: (draft: StreamDraft) => string,
) {
  const [announcement, setAnnouncement] = React.useState("");
  const previousRef = React.useRef<Map<string, StreamDraft>>(new Map());
  const labelForRef = React.useRef(labelFor);
  labelForRef.current = labelFor;

  React.useEffect(() => {
    const previous = previousRef.current;
    const next = new Map(visibleDrafts.map((draft) => [draft.key, draft]));
    const stillPresent = new Set(allDrafts.map((draft) => draft.key));
    const messages: string[] = [];
    for (const draft of visibleDrafts) {
      if (!previous.has(draft.key)) {
        messages.push(`${labelForRef.current(draft)} is replying`);
      }
    }
    for (const [key, draft] of previous) {
      if (next.has(key)) continue;
      // Hidden because its real row rendered, or ended as complete. A ghost
      // that was abandoned or expired leaves silently.
      if (stillPresent.has(key) || draft.completedAtMs !== null) {
        messages.push(`${labelForRef.current(draft)} finished replying`);
      }
    }
    previousRef.current = next;
    if (messages.length > 0) setAnnouncement(messages.join(". "));
  }, [allDrafts, visibleDrafts]);

  React.useEffect(() => {
    if (!announcement) return;
    const timeout = window.setTimeout(
      () => setAnnouncement(""),
      ANNOUNCEMENT_CLEAR_MS,
    );
    return () => window.clearTimeout(timeout);
  }, [announcement]);

  return announcement;
}

/**
 * Live reply "ghost" rows rendered after a surface's last real row. A ghost
 * with text renders like its author's message (dimmed, with a live badge);
 * a status-only ghost renders as an enhanced typing line. Ghosts are keyed by
 * stream so a status line morphs into the text ghost without remounting, and
 * they disappear in the same commit that renders their real reply.
 */
export const StreamDraftRows = React.memo(function StreamDraftRows({
  drafts,
  renderedMessages,
  currentPubkey,
  profiles,
  className,
}: StreamDraftRowsProps) {
  const visibleDrafts = React.useMemo(() => {
    if (drafts.length === 0) return drafts;
    const rendered = renderedMessages.flatMap((message) => {
      const projected = timelineRowCompletionMessage(message);
      return projected ? [projected] : [];
    });
    return selectUnrenderedStreamDrafts(drafts, rendered);
  }, [drafts, renderedMessages]);
  const labelFor = React.useCallback(
    (draft: StreamDraft) => draftAuthorLabel(draft, profiles, currentPubkey),
    [currentPubkey, profiles],
  );
  const announcement = useStreamDraftAnnouncement(
    drafts,
    visibleDrafts,
    labelFor,
  );

  return (
    <div className={cn("flex flex-col", className)} data-testid="stream-drafts">
      <p aria-live="polite" className="sr-only" data-testid="stream-draft-live">
        {announcement}
      </p>
      {visibleDrafts.map((draft) => (
        <StreamDraftRow
          author={labelFor(draft)}
          draft={draft}
          key={draft.key}
          profiles={profiles}
        />
      ))}
    </div>
  );
});

function StreamDraftRow({
  author,
  draft,
  profiles,
}: {
  author: string;
  draft: StreamDraft;
  profiles?: UserProfileLookup;
}) {
  const profile = profiles?.[normalizePubkey(draft.pubkey)];
  const isAgent = profile?.isAgent ?? false;
  const safeContent = React.useMemo(
    () => closePartialMarkdown(draft.content),
    [draft.content],
  );
  const hasContent = draft.content.trim().length > 0;
  // The status line's small avatar is centered in the message avatar column
  // so it lines up with the row it turns into.
  const avatar = (
    <div
      aria-hidden
      className={cn("shrink-0", !hasContent && "flex w-9 justify-center")}
    >
      <UserAvatar
        avatarUrl={profile?.avatarUrl ?? null}
        displayName={author}
        shape={isAgent ? "squircle" : "circle"}
        size={hasContent ? undefined : "xs"}
      />
    </div>
  );

  if (!hasContent) {
    return (
      <div
        className="flex items-center gap-2.5 px-2 py-conversation-row"
        data-stream-status={draft.status}
        data-testid="stream-draft-status"
      >
        {avatar}
        <p className="min-w-0 truncate text-xs font-medium text-muted-foreground">
          <Shimmer>{formatStreamDraftActivity(author, draft)}</Shimmer>
        </p>
      </div>
    );
  }

  return (
    <div
      className="flex items-start gap-2.5 rounded-2xl px-2 py-conversation-row"
      data-stream-status={draft.status}
      data-testid="stream-draft-row"
    >
      {avatar}
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-1.5 leading-message-author">
          <MessageAuthorText>{author}</MessageAuthorText>
          <span
            className="inline-flex items-center gap-1 rounded-full bg-muted px-1.5 text-2xs font-medium text-muted-foreground"
            data-testid="stream-draft-badge"
          >
            <span
              aria-hidden
              className="h-1.5 w-1.5 rounded-full bg-primary motion-safe:animate-pulse"
            />
            {draft.status === "tool" && draft.label
              ? `Running ${draft.label}…`
              : "Writing…"}
          </span>
        </div>
        <div
          className="mt-conversation-body opacity-70"
          data-testid="stream-draft-body"
        >
          <Markdown
            blockCode
            className="max-w-full text-message"
            content={safeContent}
            interactive={false}
          />
        </div>
      </div>
    </div>
  );
}
