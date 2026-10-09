import { toast } from "sonner";

import type { ShowProgress } from "./managedAgentControlActions";

/** A loading toast for a long agent operation (e.g. waiting for a machine
 * to confirm); dismissed when the operation settles. */
export const showProgressToast: ShowProgress = (message) => {
  const id = toast.loading(message);
  return () => {
    toast.dismiss(id);
  };
};
