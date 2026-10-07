/**
 * Screens a community window does not show: app settings belong to the main
 * window (they drive app-global and native workspace state). A shortcut or
 * link can still navigate a community window there; it renders a pointer
 * back to the main window instead. Agents, projects, and workflows run for
 * the window's own community.
 */
const MAIN_WINDOW_ONLY_ROOTS = ["settings"];

/** Whether `pathname` is a main-window-only screen. */
export function isMainWindowOnlyPath(pathname: string): boolean {
  const root = pathname.split("/").filter(Boolean)[0];
  return root !== undefined && MAIN_WINDOW_ONLY_ROOTS.includes(root);
}
