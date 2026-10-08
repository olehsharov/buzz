import { OpenInMainWindowButton } from "@/features/popout/ui/PopoutChrome";

/**
 * Shown in a community window in place of a main-window-only screen (app
 * settings). The one action brings the main window forward, so the user
 * always has a way to the screen they asked for.
 */
export function MainWindowOnlyState() {
  return (
    <div
      className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 px-6 text-center"
      data-testid="community-window-main-only"
      role="status"
    >
      <h2 className="text-base font-semibold">Available in the main window</h2>
      <p className="max-w-sm text-sm text-muted-foreground">
        Settings are managed from the main Buzz window.
      </p>
      <OpenInMainWindowButton destination={null} focusOnly />
    </div>
  );
}
