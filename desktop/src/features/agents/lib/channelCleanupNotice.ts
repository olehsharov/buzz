import { toast } from "sonner";

import {
  type ChannelCleanupReport,
  describeChannelCleanupProblem,
} from "@/shared/api/channelCleanup";

/** Tell the user about any channel a deleted agent is still in. Stays up
 * until dismissed: it names channels they have to clean up by hand. */
export function warnAboutChannelCleanup(
  subject: string,
  report: ChannelCleanupReport | undefined,
) {
  const problem = report
    ? describeChannelCleanupProblem(subject, report)
    : null;
  if (problem) toast.warning(problem, { duration: Number.POSITIVE_INFINITY });
}
