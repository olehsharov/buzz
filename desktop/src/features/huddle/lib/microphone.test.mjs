import assert from "node:assert/strict";
import test from "node:test";

import { acquireMicrophone } from "./microphone.ts";

const BASE = { echoCancellation: true, sampleRate: 48000 };

function fakeGetUserMedia(results) {
  const calls = [];
  const getUserMedia = async (constraints) => {
    calls.push(constraints);
    const next = results.shift();
    if (next instanceof Error) {
      throw next;
    }
    return next;
  };
  return { calls, getUserMedia };
}

function overconstrained() {
  // WebKit's OverconstrainedError is not a DOMException; match by name.
  const error = new Error("Invalid constraint");
  error.name = "OverconstrainedError";
  return error;
}

test("uses the system default when no device is selected", async () => {
  const stream = { id: "default" };
  const { calls, getUserMedia } = fakeGetUserMedia([stream]);

  const result = await acquireMicrophone(getUserMedia, BASE, "");

  assert.deepEqual(result, { stream, fellBackToDefault: false });
  assert.deepEqual(calls, [{ audio: BASE }]);
});

test("requests the selected device exactly", async () => {
  const stream = { id: "usb" };
  const { calls, getUserMedia } = fakeGetUserMedia([stream]);

  const result = await acquireMicrophone(getUserMedia, BASE, "usb-mic");

  assert.deepEqual(result, { stream, fellBackToDefault: false });
  assert.deepEqual(calls, [
    { audio: { ...BASE, deviceId: { exact: "usb-mic" } } },
  ]);
});

for (const [label, makeError] of [
  ["OverconstrainedError", overconstrained],
  ["NotFoundError", () => new DOMException("gone", "NotFoundError")],
]) {
  test(`falls back to the default once on ${label} for a selected device`, async () => {
    const stream = { id: "default" };
    const { calls, getUserMedia } = fakeGetUserMedia([makeError(), stream]);

    const result = await acquireMicrophone(getUserMedia, BASE, "unplugged");

    assert.deepEqual(result, { stream, fellBackToDefault: true });
    assert.deepEqual(calls, [
      { audio: { ...BASE, deviceId: { exact: "unplugged" } } },
      { audio: BASE },
    ]);
  });
}

test("propagates the default device's failure after one fallback", async () => {
  const denied = new DOMException("denied", "NotAllowedError");
  const { calls, getUserMedia } = fakeGetUserMedia([overconstrained(), denied]);

  await assert.rejects(
    acquireMicrophone(getUserMedia, BASE, "unplugged"),
    (error) => error === denied,
  );
  assert.equal(calls.length, 2);
});

test("does not retry a permission denial on the selected device", async () => {
  const denied = new DOMException("denied", "NotAllowedError");
  const { calls, getUserMedia } = fakeGetUserMedia([denied]);

  await assert.rejects(
    acquireMicrophone(getUserMedia, BASE, "usb-mic"),
    (error) => error === denied,
  );
  assert.equal(calls.length, 1);
});
