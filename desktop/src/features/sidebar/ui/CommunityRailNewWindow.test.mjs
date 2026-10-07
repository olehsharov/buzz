import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { createNewWindowGestures } from "../../popout/useOpenInNewWindow.ts";
import {
  communityRailNewWindowDestination,
  mergeCommunityRailHandlers,
} from "./CommunityRail.tsx";

const ACTIVE = "active-community";
const OTHER = "other-community";

function event(overrides = {}) {
  const calls = { prevented: 0, stopped: 0 };
  return {
    calls,
    event: {
      button: 0,
      key: "",
      metaKey: false,
      ctrlKey: false,
      preventDefault: () => {
        calls.prevented += 1;
      },
      stopPropagation: () => {
        calls.stopped += 1;
      },
      ...overrides,
    },
  };
}

/** The rail button's merged handlers, wired to a log instead of the app. */
function rig({ mac = true, communityId = OTHER } = {}) {
  const log = [];
  const enabled =
    communityRailNewWindowDestination(communityId, ACTIVE) !== null;
  const newWindow = createNewWindowGestures(
    enabled,
    () => {
      log.push("open");
      return true;
    },
    mac,
  );
  const handlers = mergeCommunityRailHandlers({
    dragListeners: {
      onKeyDown: () => log.push("drag-key"),
      onMouseDown: () => log.push("drag-mouse"),
    },
    newWindow,
    onSwitch: () => log.push("switch"),
  });
  return { handlers, log };
}

describe("community rail: open a community in its own window", () => {
  it("names no new-window destination for the active community", () => {
    assert.equal(communityRailNewWindowDestination(ACTIVE, ACTIVE), null);
    assert.deepEqual(communityRailNewWindowDestination(OTHER, ACTIVE), {
      kind: "community",
      communityId: OTHER,
    });
  });

  it("plain click switches; Cmd-click (macOS) opens a window without switching", () => {
    const { handlers, log } = rig();
    handlers.onClick(event().event);
    assert.deepEqual(log, ["switch"]);
    log.length = 0;
    const cmd = event({ metaKey: true });
    handlers.onClick(cmd.event);
    assert.deepEqual(log, ["open"]);
    assert.equal(cmd.calls.prevented, 1);
  });

  it("Ctrl-click opens on Windows/Linux but is a secondary click on macOS", () => {
    const linux = rig({ mac: false });
    linux.handlers.onClick(event({ ctrlKey: true }).event);
    assert.deepEqual(linux.log, ["open"]);
    const mac = rig({ mac: true });
    mac.handlers.onClick(event({ ctrlKey: true }).event);
    assert.deepEqual(mac.log, ["switch"]);
  });

  it("middle click opens and suppresses autoscroll; dnd still sees the mousedown", () => {
    const { handlers, log } = rig();
    const down = event({ button: 1 });
    handlers.onMouseDown(down.event);
    assert.equal(down.calls.prevented, 1);
    assert.deepEqual(log, ["drag-mouse"]);
    log.length = 0;
    handlers.onAuxClick(event({ button: 1 }).event);
    assert.deepEqual(log, ["open"]);
  });

  it("Cmd+Enter opens and never reaches dnd-kit's keyboard drag start", () => {
    const { handlers, log } = rig();
    handlers.onKeyDown(event({ key: "Enter", metaKey: true }).event);
    assert.deepEqual(log, ["open"]);
    log.length = 0;
    // Plain Enter/Space still belong to the sortable keyboard sensor.
    handlers.onKeyDown(event({ key: "Enter" }).event);
    handlers.onKeyDown(event({ key: " " }).event);
    assert.deepEqual(log, ["drag-key", "drag-key"]);
  });

  it("every gesture on the active community falls through to its normal action", () => {
    const { handlers, log } = rig({ communityId: ACTIVE });
    handlers.onClick(event({ metaKey: true }).event);
    handlers.onAuxClick(event({ button: 1 }).event);
    handlers.onKeyDown(event({ key: "Enter", metaKey: true }).event);
    assert.deepEqual(log, ["switch", "drag-key"]);
  });
});
