/**
 * The huddle bar's error banner. The message wraps instead of truncating:
 * huddle failures (a community window that cannot host audio, a relay that
 * refuses the join) are only actionable when the whole sentence is readable,
 * and a hover-only `title` would make screen readers read it twice. The
 * dismiss button is the banner's only action and owns its own label.
 */
export function HuddleErrorBanner({
  message,
  onDismiss,
}: {
  message: string;
  onDismiss: () => void;
}) {
  return (
    <div
      className="flex min-w-0 items-start gap-1.5 rounded bg-destructive/10 px-2 py-1 text-xs text-destructive"
      data-testid="huddle-error-banner"
      role="alert"
    >
      <span className="min-w-0 whitespace-normal break-words">{message}</span>
      <button
        aria-label="Dismiss error"
        className="ml-1 shrink-0 opacity-60 hover:opacity-100"
        onClick={onDismiss}
        type="button"
      >
        <span aria-hidden="true">✕</span>
      </button>
    </div>
  );
}
