/**
 * Screens a community window does not show: agent management, projects,
 * workflows, and settings belong to the main window (they drive app-global
 * agents, the nest, and native workspace state). Keyboard shortcuts or links
 * can still navigate a community window there; it renders a pointer back to
 * the main window instead.
 */
const MAIN_WINDOW_ONLY_ROOTS = ["agents", "projects", "workflows", "settings"];

/** Whether `pathname` is a main-window-only screen. */
export function isMainWindowOnlyPath(pathname: string): boolean {
  const root = pathname.split("/").filter(Boolean)[0];
  return root !== undefined && MAIN_WINDOW_ONLY_ROOTS.includes(root);
}
