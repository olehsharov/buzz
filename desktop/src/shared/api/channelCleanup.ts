/** Channels a deleted agent was removed from, and any it is still in. */
export type ChannelCleanupReport = {
  removed: { id: string; name: string }[];
  failed: { channelId: string; channelName: string; error: string }[];
  /** The memberships could not be listed: the agent may still be in
   * channels nobody saw. */
  lookupError: string | null;
};

export type RawChannelCleanupReport = {
  removed?: { id: string; name: string }[];
  failed?: { channel_id: string; channel_name: string; error: string }[];
  lookup_error?: string | null;
};

export const EMPTY_CHANNEL_CLEANUP: ChannelCleanupReport = {
  removed: [],
  failed: [],
  lookupError: null,
};

export function fromRawChannelCleanup(
  raw: RawChannelCleanupReport | null | undefined,
): ChannelCleanupReport {
  if (!raw) return EMPTY_CHANNEL_CLEANUP;
  return {
    removed: raw.removed ?? [],
    failed: (raw.failed ?? []).map((failure) => ({
      channelId: failure.channel_id,
      channelName: failure.channel_name,
      error: failure.error,
    })),
    lookupError: raw.lookup_error ?? null,
  };
}

/**
 * What the user must know about a delete's channel cleanup, or null when
 * every membership was removed. A channel the agent is still in is never
 * silent: it names each channel and why.
 */
export function describeChannelCleanupProblem(
  subject: string,
  report: ChannelCleanupReport,
): string | null {
  const problems: string[] = [];
  if (report.failed.length > 0) {
    problems.push(
      `${subject} is still a member of ${report.failed
        .map((failure) => `#${failure.channelName} (${failure.error})`)
        .join(", ")}. Remove it from the channel's member list.`,
    );
  }
  if (report.lookupError) {
    problems.push(
      `Buzz could not check which channels ${subject} was in (${report.lookupError}); it may still be listed as a member.`,
    );
  }
  return problems.length > 0 ? problems.join(" ") : null;
}

/** Fold per-agent channel reports into one. */
export function mergeChannelCleanup(
  reports: readonly (ChannelCleanupReport | undefined)[],
): ChannelCleanupReport | undefined {
  const present = reports.filter(
    (report): report is ChannelCleanupReport => report !== undefined,
  );
  if (present.length === 0) return undefined;
  const lookupErrors = present
    .map((report) => report.lookupError)
    .filter((error): error is string => Boolean(error));
  return {
    removed: present.flatMap((report) => report.removed),
    failed: present.flatMap((report) => report.failed),
    lookupError: lookupErrors.length > 0 ? lookupErrors.join("; ") : null,
  };
}
