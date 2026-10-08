import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(
  new URL("./useNewMessageRecipients.ts", import.meta.url),
  "utf8",
);

test("the new-message picker admits agents through the DM recipient scope", () => {
  // buzz-acp ignores DMs from anyone but the agent's owner and the owner's
  // agents, so the picker must not use the community (channel) scope that
  // admits other people's "Anyone" agents.
  assert.match(
    source,
    /const eligibleAgentPubkeys = getDirectMessageRecipientAgentPubkeys\(\{/,
  );
  assert.doesNotMatch(source, /eligibilityScope: \{ type: "community" \}/);
});
