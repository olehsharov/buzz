import assert from "node:assert/strict";
import test from "node:test";

function installLocalStorage(entries = {}) {
  const store = new Map(Object.entries(entries));
  globalThis.localStorage = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: (key) => store.delete(key),
  };
}

const CHANNEL = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";

test("a community window opens on its community's last channel, else Home", async () => {
  installLocalStorage({
    "buzz-community-destinations": JSON.stringify({
      remembered: { kind: "channel", channelId: CHANNEL },
      "went-home": { kind: "home" },
    }),
  });
  const { buildPopoutRoute, communityLandingRoute } = await import(
    "../popout/popoutRoute.ts"
  );
  assert.equal(communityLandingRoute("remembered"), `/channels/${CHANNEL}`);
  assert.equal(communityLandingRoute("went-home"), "/");
  assert.equal(communityLandingRoute("never-visited"), "/");
  assert.equal(communityLandingRoute("bad/id"), null);
  assert.equal(
    buildPopoutRoute({ kind: "community", communityId: "remembered" }),
    `/channels/${CHANNEL}`,
  );
});

test("a community window opens pop-outs but not other community windows", async () => {
  installLocalStorage();
  const { canOpenInNewWindow } = await import(
    "../popout/useOpenInNewWindow.ts"
  );
  const channel = { kind: "channel", channelId: CHANNEL };
  assert.equal(canOpenInNewWindow(channel, false), true);
  assert.equal(canOpenInNewWindow(channel, true), true);
  assert.equal(
    canOpenInNewWindow({ kind: "community", communityId: "x" }, true),
    false,
  );
  assert.equal(
    canOpenInNewWindow({ kind: "community", communityId: "x" }, false),
    true,
  );
});
