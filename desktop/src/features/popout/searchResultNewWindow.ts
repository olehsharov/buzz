import {
  resolveSearchHitDestination,
  type SearchHitDestination,
} from "@/app/navigation/resolveSearchHitDestination";
import type { PopoutDestination } from "@/features/popout/popoutRoute";
import type { SearchResult } from "@/features/search/ui/SearchResultItem";
import type { SearchHit } from "@/shared/api/types";

type Deps = {
  openInNewWindow: (destination: PopoutDestination) => boolean;
  /** Opens (or reopens) the DM with these participants; resolves its id. */
  openDm: (input: { pubkeys: string[] }) => Promise<{ id: string }>;
  resolveDestination?: (
    hit: SearchHit,
  ) => Promise<SearchHitDestination | null | undefined>;
};

/**
 * Opens a search result's destination in a pop-out: channels and DMs
 * directly, people via their DM (the same place a plain open goes), and
 * message hits at the resolved message / thread / forum post. Create/browse
 * actions have no destination and are ignored.
 */
export async function openSearchResultInNewWindow(
  result: SearchResult,
  {
    openInNewWindow,
    openDm,
    resolveDestination = resolveSearchHitDestination,
  }: Deps,
): Promise<boolean> {
  switch (result.kind) {
    case "channel":
      return openInNewWindow({ kind: "channel", channelId: result.channel.id });
    case "user": {
      const dm = await openDm({ pubkeys: [result.user.pubkey] });
      return openInNewWindow({ kind: "channel", channelId: dm.id });
    }
    case "message": {
      const destination = await resolveDestination(result.hit);
      if (!destination) return false;
      if (destination.kind === "forum-post") {
        return openInNewWindow({
          kind: "forum-post",
          channelId: destination.channelId,
          postId: destination.postId,
          replyId: destination.replyId ?? null,
        });
      }
      return openInNewWindow({
        kind: "channel",
        channelId: destination.channelId,
        messageId: destination.messageId ?? null,
        threadRootId: destination.threadRootId ?? null,
      });
    }
    default:
      return false;
  }
}
