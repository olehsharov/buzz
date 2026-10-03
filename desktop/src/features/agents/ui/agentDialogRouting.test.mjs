import assert from "node:assert/strict";
import test from "node:test";
import React from "react";

import { AgentDialog, DefinitionCreateDialogRouter } from "./AgentDialog.tsx";
import { AgentDefinitionDialog } from "./AgentDefinitionDialog.tsx";
import { AgentInstanceEditDialog } from "./AgentInstanceEditDialog.tsx";
import { AgentRunLocationProvider } from "./AgentRunLocationContext.tsx";
import { WhereToRunSection } from "./WhereToRunSection.tsx";

// ── Phase 1B.3c routing pinning ─────────────────────────────────────────────
//
// AgentDialog is the single dialog entry point for every intent. It is
// hook-free by design, so each union arm can be exercised as a plain function
// call and the returned element inspected: the arm must route to the form
// component that owns the intent, and pass-through arms must forward the
// caller's props byte-for-byte (minus the `mode` discriminant).

const noop = () => {};

test("definition-edit routes to AgentDefinitionDialog with exact pass-through", () => {
  const props = {
    description: "Edit the agent definition.",
    error: null,
    initialValues: { id: "persona-1", displayName: "Brain" },
    isPending: false,
    onOpenChange: noop,
    onSubmit: async () => {},
    open: true,
    runtimes: [],
    runtimesLoading: false,
    submitLabel: "Save",
    title: "Edit agent",
  };

  const element = AgentDialog({ mode: "definition-edit", ...props });

  assert.equal(element.type, AgentDefinitionDialog);
  assert.deepEqual(element.props, props, "props must pass through unchanged");
  assert.equal(
    "mode" in element.props,
    false,
    "the mode discriminant must not leak into AgentDefinitionDialog",
  );
});

test("create-mode definition-edit (duplicate/import) routes to the run-location router", () => {
  const props = {
    description: "Copy this agent.",
    error: null,
    initialValues: { displayName: "Brain copy" },
    isPending: false,
    onOpenChange: noop,
    onSubmit: async () => {},
    open: true,
    runtimes: [],
    submitLabel: "Create agent",
    title: "Duplicate Brain",
  };

  const element = AgentDialog({ mode: "definition-edit", ...props });

  assert.equal(
    element.type,
    DefinitionCreateDialogRouter,
    "a definition without an id creates and starts an agent, so it must offer Where to run",
  );
  assert.deepEqual(element.props, props);
});

test("instance-edit routes to AgentInstanceEditDialog with its contract props", () => {
  const agent = { pubkey: "abc", name: "test-agent" };
  const onOpenChange = noop;
  const onUpdated = noop;

  const element = AgentDialog({
    mode: "instance-edit",
    agent,
    onOpenChange,
    onUpdated,
    open: true,
  });

  // The arm wraps the form in the run-location provider so the respond-to
  // warning can name the machine without the value being threaded as a prop
  // through AgentInstanceEditDialog (see AgentRunLocationContext for why).
  assert.equal(element.type, AgentRunLocationProvider);
  const form = element.props.children;
  assert.equal(form.type, AgentInstanceEditDialog);
  assert.deepEqual(form.props, {
    agent,
    onEditLinkedPersona: undefined,
    onOpenChange,
    onUpdated,
    open: true,
    initialFocus: undefined,
  });
});

test("instance-edit publishes the run location resolved from the agent backend", () => {
  const routeWithBackend = (backend) =>
    AgentDialog({
      mode: "instance-edit",
      agent: { pubkey: "abc", name: "test-agent", backend },
      onOpenChange: noop,
      onUpdated: noop,
      open: true,
    }).props.runLocation;

  assert.equal(routeWithBackend({ type: "local" }), "local");
  assert.equal(
    routeWithBackend({ type: "provider", id: "blox", config: {} }),
    "remote",
  );
  // An agent with no backend record has an unknown location — never a guess.
  assert.equal(routeWithBackend(undefined), null);
});

test("create mode routes to the internal create router, not a form directly", () => {
  const element = AgentDialog({
    mode: "definition",
    definitionError: null,
    isDefinitionPending: false,
    onOpenChange: noop,
    onSubmitDefinition: async () => true,
    runtimes: [],
    runtimesLoading: false,
  });

  assert.notEqual(element.type, AgentDefinitionDialog);
  assert.notEqual(element.type, AgentInstanceEditDialog);
  assert.equal(
    typeof element.type,
    "function",
    "definition must route through the internal create router",
  );
  assert.equal(element.type.name, "AgentCreateDialogRouter");
});

// ── Agent-draft prefill seam ────────────────────────────────────────────────

const draftedRunDraft = {
  runOn: "blox",
  providerConfig: { workdir: "/srv/agents" },
  probedProvider: null,
};

function createArmProps(overrides = {}) {
  return {
    mode: "definition",
    definitionError: null,
    isDefinitionPending: false,
    onOpenChange: noop,
    onSubmitDefinition: async () => true,
    runtimes: [],
    runtimeCatalogStatus: "ready",
    ...overrides,
  };
}

/**
 * Render one pass of a hook-using component as a plain call: a minimal hooks
 * dispatcher answers useState/useMemo with their initial values, which is all
 * the create router's first render needs.
 */
function renderFirstPass(Component, props) {
  const internals =
    React.__CLIENT_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE;
  const previous = internals.H;
  internals.H = {
    useState: (initial) => [
      typeof initial === "function" ? initial() : initial,
      noop,
    ],
    useMemo: (factory) => factory(),
  };
  try {
    return Component(props);
  } finally {
    internals.H = previous;
  }
}

test("definition create arm forwards the drafted run draft and notices", () => {
  const reviewNotices = ["This draft asks to run on “blox”."];
  const element = AgentDialog(
    createArmProps({ initialRunDraft: draftedRunDraft, reviewNotices }),
  );

  assert.equal(element.type.name, "AgentCreateDialogRouter");
  assert.equal(element.props.initialRunDraft, draftedRunDraft);
  assert.equal(element.props.reviewNotices, reviewNotices);
});

test("the create router seeds Run on from initialRunDraft", () => {
  const reviewNotices = ["note"];
  const Router = AgentDialog(createArmProps()).type;
  const tree = renderFirstPass(
    Router,
    createArmProps({ initialRunDraft: draftedRunDraft, reviewNotices }),
  );

  assert.equal(tree.type, AgentRunLocationProvider);
  assert.equal(tree.props.runLocation, "remote");
  const form = tree.props.children;
  assert.equal(form.type, AgentDefinitionDialog);
  assert.equal(form.props.createRunSection.type, WhereToRunSection);
  assert.equal(form.props.createRunSection.props.draft, draftedRunDraft);
  assert.equal(form.props.createNotices, reviewNotices);
  // Not probed yet, so the provider's config cannot be submitted.
  assert.equal(form.props.createSubmitBlocked, true);
});

test("the create router starts on this computer without a drafted run draft", () => {
  const Router = AgentDialog(createArmProps()).type;
  for (const initialRunDraft of [undefined, null]) {
    const tree = renderFirstPass(Router, createArmProps({ initialRunDraft }));
    const form = tree.props.children;
    assert.deepEqual(form.props.createRunSection.props.draft, {
      runOn: "local",
      providerConfig: {},
      probedProvider: null,
    });
    assert.equal(form.props.createSubmitBlocked, false);
  }
});
