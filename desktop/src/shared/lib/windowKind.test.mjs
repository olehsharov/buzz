import assert from "node:assert/strict";
import test from "node:test";

import {
  communityIdFromWindowLabel,
  isCommunityWindowId,
  windowKindFromLabel,
} from "./windowKind.ts";

const UUID = "0b6c3a9e-2f5f-4f9e-9a43-0b8b2a6f3c11";

test("windowKindFromLabel classifies every window role", () => {
  assert.equal(windowKindFromLabel(null), "main");
  assert.equal(windowKindFromLabel("main"), "main");
  assert.equal(windowKindFromLabel(`huddle-${UUID}`), "huddle");
  assert.equal(windowKindFromLabel(`popout-${UUID}`), "popout");
  assert.equal(windowKindFromLabel(`community-${UUID}`), "community");
  assert.equal(
    windowKindFromLabel("community-e2e-default-community"),
    "community",
  );
});

test("malformed community labels fall back to the main window", () => {
  for (const label of ["community-", "community-a/b", "community-a.b"]) {
    assert.equal(windowKindFromLabel(label), "main", label);
    assert.equal(communityIdFromWindowLabel(label), null, label);
  }
  assert.equal(
    communityIdFromWindowLabel(`community-${"a".repeat(129)}`),
    null,
  );
});

test("communityIdFromWindowLabel names the bound community", () => {
  assert.equal(communityIdFromWindowLabel(`community-${UUID}`), UUID);
  assert.equal(communityIdFromWindowLabel(`popout-${UUID}`), null);
  assert.equal(communityIdFromWindowLabel(null), null);
  assert.equal(isCommunityWindowId(UUID), true);
  assert.equal(isCommunityWindowId("../x"), false);
});
