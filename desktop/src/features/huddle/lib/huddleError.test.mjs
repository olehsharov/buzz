import assert from "node:assert/strict";
import test from "node:test";

import { formatHuddleActionError } from "./huddleError.ts";

const AUDIO_UNAVAILABLE_MESSAGE =
  "Huddle audio isn’t available on this server. Ask an administrator to turn it on.";

test("maps the relay deployment rejection to actionable copy", () => {
  assert.equal(
    formatHuddleActionError(
      "audio relay auth error: huddle audio unavailable in this deployment",
      "join",
    ),
    AUDIO_UNAVAILABLE_MESSAGE,
  );
});

test("recognizes the relay error code when present", () => {
  assert.equal(
    formatHuddleActionError("huddle_audio_unavailable", "start"),
    AUDIO_UNAVAILABLE_MESSAGE,
  );
});

test("preserves other string and Error messages", () => {
  assert.equal(
    formatHuddleActionError("Microphone unavailable", "join"),
    "Microphone unavailable",
  );
  assert.equal(
    formatHuddleActionError(new Error("Connection timed out"), "start"),
    "Connection timed out",
  );
});

test("maps a denied microphone to platform-specific settings copy", () => {
  for (const name of ["NotAllowedError", "SecurityError"]) {
    const error = new DOMException("Permission denied", name);
    assert.equal(
      formatHuddleActionError(error, "join", true),
      "Buzz can’t use your microphone. Turn it on in System Settings → Privacy & Security → Microphone, then try again.",
    );
    assert.equal(
      formatHuddleActionError(error, "start", false),
      "Buzz can’t use your microphone. Allow microphone access for Buzz in your system settings, then try again.",
    );
  }
});

test("maps a missing microphone to actionable copy", () => {
  const overconstrained = new Error("Invalid constraint");
  overconstrained.name = "OverconstrainedError";
  for (const error of [
    new DOMException("Requested device not found", "NotFoundError"),
    overconstrained,
  ]) {
    assert.equal(
      formatHuddleActionError(error, "start", true),
      "No microphone found. Connect a microphone, then try again.",
    );
  }
});

test("maps a busy microphone to actionable copy", () => {
  assert.equal(
    formatHuddleActionError(
      new DOMException("Could not start audio source", "NotReadableError"),
      "join",
      false,
    ),
    "Your microphone is in use by another app or unavailable. Close other apps using it, then try again.",
  );
});

test("uses action-specific fallback copy for unknown errors", () => {
  assert.equal(
    formatHuddleActionError({ reason: "unknown" }, "join"),
    "Couldn’t join the huddle.",
  );
  assert.equal(
    formatHuddleActionError(null, "start"),
    "Couldn’t start the huddle.",
  );
});
