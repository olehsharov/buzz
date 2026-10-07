import * as React from "react";
import { ProtectedGlobalOverlay } from "@protected-feature-components";
import { useQueryClient } from "@tanstack/react-query";
import { Outlet, useLocation } from "@tanstack/react-router";
import {
  appShellWindowRoles,
  deriveShellRoute,
  markAllReadSources,
} from "@/app/AppShell.helpers";
import { useTerminalContext } from "@/app/useTerminalContext";
import { AppShellProvider } from "@/app/AppShellContext";
import { AppShellOverlays, TerminalBootstrap } from "@/app/AppShellOverlays";
import { AppShellChannelSurface } from "@/app/AppShellChannelSurface";
import { AppHuddleShell } from "@/app/AppHuddleShell";
import { AppTopChrome } from "@/app/AppTopChrome";
import {
  type TerminalContextOverride,
  TerminalContextOverrideProvider,
} from "@/app/TerminalContextOverrideContext";
import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useBackForwardControls } from "@/app/navigation/useBackForwardControls";
import { useCommunityNavigationTransitions } from "@/app/useCommunityNavigationTransitions";
import { useLiveHomeFeedActions } from "@/app/useLiveHomeFeedActions";
import { useChannelBrowserDialog } from "@/app/useChannelBrowserDialog";
import { useMarkAsReadShortcuts } from "@/app/useMarkAsReadShortcuts";
import { useSettingsShortcuts } from "@/app/useSettingsShortcuts";
import { useAppShellKeyboardShortcuts } from "@/app/useAppShellKeyboardShortcuts";
import { useAppShellDesktopNotifications } from "@/app/useAppShellDesktopNotifications";
import { useAppShellLifecycleEffects } from "@/app/useAppShellLifecycleEffects";
import { useChannelActivityProjection } from "@/app/useChannelActivityProjection";
import { useTauriWindowDrag } from "@/app/useTauriWindowDrag";
import { useWebviewZoomShortcuts } from "@/app/useWebviewZoomShortcuts";
import { useHuddlePresentation } from "@/app/useHuddlePresentation";
import { shouldShowSidebarChannel } from "@/app/huddleChannelVisibility";
import {
  channelsQueryKey,
  useChannelsQuery,
  useCreateChannelMutation,
  useHideDmMutation,
  useOpenDmMutation,
} from "@/features/channels/hooks";
import { useDmResurfaceFromMessages } from "@/features/channels/useDmResurfaceFromMessages";
import { useUnreadChannels } from "@/features/channels/useUnreadChannels";
import { useMembershipNotifications } from "@/features/channels/useMembershipNotifications";
import { useFeedItemState } from "@/features/home/useFeedItemState";
import { useThreadFollows } from "@/features/messages/lib/useThreadFollows";
import {
  useHomeFeedNotifications,
  useHomeFeedNotificationState,
} from "@/features/notifications/hooks";
import { PreventSleepProvider } from "@/features/agents/usePreventSleep";
import { requestOpenCreateAgent } from "@/features/agents/openCreateAgentEvent";
import { useAgentsDataRefresh } from "@/features/agents/lib/useAgentsDataRefresh";
import { useManagedAgentRuntimeReconciliation } from "@/features/agents/useManagedAgentRuntimeReconciliation";
import { useAutoRestartPolicy } from "@/features/agents/lib/useAutoRestartPolicy";
import { usePersonaSync } from "@/features/agents/lib/usePersonaSync";
import { useAgentObserverIngestion } from "@/features/agents/useAgentObserverIngestion";
import { useAgentHostTelemetry } from "@/features/agents/hosts/useAgentHosts";
import { AgentManagementDialogs } from "@/features/agents/ui/AgentManagementDialogs";
import { RequestedAgentCreateDialogs } from "@/features/agents/ui/RequestedAgentCreateDialogs";
import {
  usePresenceSession,
  usePresenceSubscription,
} from "@/features/presence/hooks";
import {
  useSetUserStatusMutation,
  useUserStatusQuery,
  visibleUserStatus,
  useUserStatusSubscription,
} from "@/features/user-status/hooks";
import { useCommunityEmojiLiveUpdates } from "@/features/custom-emoji/hooks";
import { useArchiveSync } from "@/features/local-archive/useArchiveSync";
import { useArchiveAgentMetricsBridge } from "@/features/local-archive/useArchiveAgentMetricsBridge";
import { useObserverArchiveReconciliation } from "@/features/local-archive/useObserverArchiveSeed";
import { useAgentMetricArchiveSeed } from "@/features/local-archive/useAgentMetricArchiveSeed";
import { useProfileQuery } from "@/features/profile/hooks";
import { SendFeedbackController } from "@/features/settings/ui/SendFeedbackController";
import {
  DEFAULT_SETTINGS_SECTION,
  type SettingsSection,
  isSettingsSection,
} from "@/features/settings/ui/SettingsPanels";
import { useDueReminderBadgeCount } from "@/features/reminders/hooks";
import { useReminderNotifications } from "@/features/reminders/useReminderNotifications";
import { AppSidebar } from "@/features/sidebar/ui/AppSidebar";
import { requestFocusedThreadClose } from "@/features/channels/focusedThreadCloseRequest";
import { CommunityRail } from "@/features/sidebar/ui/CommunityRail";
import { useChannelMutes } from "@/features/sidebar/lib/useChannelMutes";
import { useChannelStars } from "@/features/sidebar/lib/useChannelStars";
import { useCommunities } from "@/features/communities/useCommunities";
import {
  consumePendingCommunityRestore,
  loadCommunityDestination,
  saveCommunityDestination,
} from "@/features/communities/communityNavigationStorage";
import { useAddCommunityDialogState } from "@/features/communities/addCommunityPrefill";
import { useApplyTemplate } from "@/features/channel-templates/useApplyTemplate";
import { relayClient } from "@/shared/api/relayClient";
import { useIdentityQuery } from "@/shared/api/hooks";
import { useRelayAutoHeal } from "@/shared/api/useRelayAutoHeal";
import { useDeferredStartup } from "@/shared/hooks/useDeferredStartup";
import { useWebviewScrollBoundaryLock } from "@/shared/hooks/useWebviewScrollBoundaryLock";
import { joinChannel } from "@/shared/api/tauri";
import type { Channel, ChannelVisibility, SearchHit } from "@/shared/api/types";
import { ChannelNavigationProvider } from "@/shared/context/ChannelNavigationContext";
import { useAppDeepLinks } from "@/shared/useAppDeepLinks";
import { SidebarProvider } from "@/shared/ui/sidebar";
import { RelayConnectionOverlay } from "@/app/RelayConnectionOverlay";
import { useSidebarRelayConnectionCard } from "@/features/sidebar/ui/useSidebarRelayConnectionCard";
import { AppShellTrayMenu } from "@/app/useAppShellTrayMenu";
import { AppProfilePanelProvider } from "@/app/AppProfilePanelProvider";
import { AppWorkflowEditorOverlayProvider } from "@/app/AppWorkflowEditorOverlayProvider";
import { LazySettingsScreen } from "@/app/LazySettingsScreen";
import {
  openInMainWindow,
  PopoutHeader,
  PopoutUnavailableState,
} from "@/features/popout/ui/PopoutChrome";
import { useMainWindowPopoutNavigation } from "@/features/popout/useMainWindowPopoutNavigation";
import { openSearchResultInNewWindow } from "@/features/popout/searchResultNewWindow";
import type { SearchResult } from "@/features/search/ui/SearchResultItem";
import { currentWindowKind } from "@/shared/lib/windowKind";
import { isMainWindowOnlyPath } from "@/features/community-window/communityWindowRoutes";
import { MainWindowOnlyState } from "@/features/community-window/ui/MainWindowOnlyState";
const EMPTY_CHANNELS: Channel[] = [];
const EMPTY_COMMUNITIES: never[] = [];
export function AppShell() {
  useWebviewZoomShortcuts();
  useTauriWindowDrag();
  useWebviewScrollBoundaryLock();
  const communitiesHook = useCommunities();
  const {
    handleHuddleCompanionOpen,
    handleHuddleEnded,
    handleHuddleStartPendingChange,
    handleHuddleStarted,
    handleHuddleVisibilityChange,
    handleSidebarChannelSelect,
    huddleBackingChannelIds,
    revealedHuddleChannelIds,
    isHuddleCompanionOpen,
    isHuddleDrawerOpen,
    isHuddleRoom,
    isHuddleRoomStarting,
    showHuddleInMainApp,
    viewHuddleChannel,
  } = useHuddlePresentation();
  // Huddle companions and pop-outs load this same shell but own no
  // app-global work: unread tracking, notifications, badge, deep links,
  // settings, sidebar, and community rail stay with the main window.
  // A community window shows the whole chat surface of its own community
  // (sidebar, channels, composer, search, profiles) but, like a pop-out,
  // owns no app-global work and no agent/workflow/project management.
  const {
    isPopout,
    isAuxWindow,
    isCommunityWindowShell,
    isSecondaryWindow,
    ownsAppGlobals,
  } = appShellWindowRoles(currentWindowKind(), isHuddleRoom);
  const hasCommunityRail = communitiesHook.communities.length > 1;
  const addCommunityDialog = useAddCommunityDialogState();
  const [isChannelManagementOpen, setIsChannelManagementOpen] =
    React.useState(false);
  const [managedChannelId, setManagedChannelId] = React.useState<string | null>(
    null,
  );
  const [searchFocusRequest, setSearchFocusRequest] = React.useState(0);
  const [scopeSearchFocusRequest, setScopeSearchFocusRequest] =
    React.useState(0);
  const [isCreateChannelOpen, setIsCreateChannelOpen] = React.useState(false);
  const [isSendFeedbackOpen, setIsSendFeedbackOpen] = React.useState(false);
  const mainInsetRef = React.useRef<HTMLElement>(null);
  const location = useLocation();
  const queryClient = useQueryClient();
  useManagedAgentRuntimeReconciliation(
    isSecondaryWindow ? EMPTY_COMMUNITIES : communitiesHook.communities,
  ); // sync storage snapshot
  const {
    goAgents,
    goChannel,
    goHome,
    goNewMessage,
    goProjects,
    goPulse,
    goSettings,
    goWorkflows,
    closeSettings,
    openInNewWindow,
    openSearchHit,
  } = useAppNavigation();
  const { canGoBack, canGoForward, goBack, goForward } =
    useBackForwardControls();
  const { selectedChannelId, selectedView } = React.useMemo(
    () => deriveShellRoute(location.pathname),
    [location.pathname],
  );
  const {
    leaveAndRemoveCommunity: handleLeaveCommunity,
    removeCommunityFromDevice: removeFromDevice,
    switchCommunity: handleSwitchCommunity,
  } = useCommunityNavigationTransitions({
    communities: communitiesHook,
    goHome,
    selectedChannelId,
    selectedView,
  });
  // Settings lives in history so back returns to the previous app entry.
  const settingsOpen = !isSecondaryWindow && location.pathname === "/settings";
  const rawLocationSearchSection = (location.search as { section?: unknown })
    .section;
  // Migrate the legacy "moderation" token to "relay-admin" (renamed section
  // id). useLocation().search is the raw URL query, bypassing route validation,
  // so the alias must be applied here too. The "doctor" alias is NOT needed on
  // this path: AppShell never carried it on main, and it lives only in
  // validateSettingsSearch where route validation rewrites it before rendering.
  const locationSearchSection =
    rawLocationSearchSection === "moderation"
      ? "relay-admin"
      : rawLocationSearchSection;
  const settingsSection: SettingsSection = isSettingsSection(
    locationSearchSection,
  )
    ? locationSearchSection
    : DEFAULT_SETTINGS_SECTION;
  const startupReady = useDeferredStartup();
  const identityQuery = useIdentityQuery();
  const { mutedChannelIds, muteChannel, unmuteChannel } = useChannelMutes(
    identityQuery.data?.pubkey,
    communitiesHook.activeCommunity?.relayUrl,
  );
  const { starredChannelIds, starChannel, unstarChannel } = useChannelStars(
    identityQuery.data?.pubkey,
    communitiesHook.activeCommunity?.relayUrl,
  );
  usePersonaSync(
    isSecondaryWindow ? undefined : identityQuery.data?.pubkey,
    communitiesHook.activeCommunity?.relayUrl,
  );
  useAgentsDataRefresh();
  // Chunk F: auto-restart drifted idle agents (per-agent opt-out, default ON).
  useAutoRestartPolicy(
    communitiesHook.activeCommunity?.relayUrl,
    !isSecondaryWindow,
  );
  // Owner-global observer ingestion: receives + decrypts agent observer
  // frames and keeps derived active-turn liveness in sync app-wide, so no
  // individual screen/panel has to mount its own bridge for ingestion.
  // Intentionally mounted without a `startupReady`/identity guard: before
  // `currentPubkey` resolves the hook ingests managed agents only, and
  // relay-owned agents join automatically once identity arrives. Adding a
  // guard here would drop managed-agent coverage during startup.
  useAgentObserverIngestion();
  useAgentHostTelemetry();
  // Kind 24200 is relay-ephemeral, so reconciliation runs eagerly (not
  // deferred): seeds kind 24200 for fresh identities, no-ops for explicit
  // opt-outs. Frames before the listener opens are permanently lost.
  const observerReconciled = useObserverArchiveReconciliation(
    isSecondaryWindow ? undefined : identityQuery.data?.pubkey,
  );
  // useArchiveSync must wait for reconciliation, or listeners could open
  // before kind 24200 is guaranteed present in the subscription.
  useArchiveSync(observerReconciled);
  // The archive batch now persists in Rust, so the agent-metrics invalidation
  // signal arrives as a Tauri event rather than an in-process call.
  useArchiveAgentMetricsBridge();
  // Kind 44200 is relay-persisted (durable) and stays deferred: missed
  // startup frames can be replayed, so there's no ordering constraint.
  const deferredPubkey = startupReady ? identityQuery.data?.pubkey : undefined;
  useAgentMetricArchiveSeed(isSecondaryWindow ? undefined : deferredPubkey);
  const profileQuery = useProfileQuery();
  useRelayAutoHeal();
  usePresenceSubscription();
  useUserStatusSubscription();
  useCommunityEmojiLiveUpdates();
  useMembershipNotifications(identityQuery.data?.pubkey);
  // Presence is published by the main window only.
  const presenceSession = usePresenceSession(
    isSecondaryWindow ? undefined : deferredPubkey,
  );
  const selfStatusQuery = useUserStatusQuery(
    deferredPubkey ? [deferredPubkey] : [],
  );
  const setUserStatusMutation = useSetUserStatusMutation(deferredPubkey);
  const { feedProfilesQuery, homeFeedQuery, notificationSettings } =
    useHomeFeedNotifications(identityQuery.data?.pubkey);
  const feedItemState = useFeedItemState(identityQuery.data?.pubkey);
  const channelsQuery = useChannelsQuery();
  const channels = channelsQuery.data ?? [];
  useReminderNotifications(
    isSecondaryWindow ? undefined : identityQuery.data?.pubkey,
    notificationSettings.settings,
    channels,
  );
  const refetchHomeFeedFromLiveSignal = React.useEffectEvent(() => {
    void homeFeedQuery.refetch();
  });
  useLiveHomeFeedActions(
    identityQuery.data?.pubkey,
    refetchHomeFeedFromLiveSignal,
  );
  const { refetch: refetchChannels } = channelsQuery;
  const channelsErrorMessage =
    channelsQuery.error instanceof Error
      ? channelsQuery.error.message
      : undefined;
  const relayConnectionCard = useSidebarRelayConnectionCard(
    channelsErrorMessage,
    communitiesHook.activeCommunity?.relayUrl,
    `${communitiesHook.activeCommunity?.id ?? "none"}-${communitiesHook.reinitKey}`,
  );
  const memberChannels = React.useMemo(
    () => channels.filter((channel) => channel.isMember),
    [channels],
  );
  const sidebarChannels = React.useMemo(
    () =>
      memberChannels.filter(
        (channel) =>
          channel.archivedAt === null &&
          shouldShowSidebarChannel(
            channel,
            huddleBackingChannelIds,
            revealedHuddleChannelIds,
          ),
      ),
    [huddleBackingChannelIds, memberChannels, revealedHuddleChannelIds],
  );
  const hasRestoredCommunityDestinationRef = React.useRef(false);
  React.useEffect(() => {
    const activeCommunityId = communitiesHook.activeCommunity?.id;
    if (
      isSecondaryWindow ||
      hasRestoredCommunityDestinationRef.current ||
      !channelsQuery.isSuccess ||
      channelsQuery.dataUpdatedAt === 0 ||
      !activeCommunityId
    ) {
      return;
    }
    hasRestoredCommunityDestinationRef.current = true;

    // Restoration belongs to an explicit community transition. Cold boot and
    // reconnect remounts must preserve the route the user explicitly opened.
    if (!consumePendingCommunityRestore(activeCommunityId)) {
      return;
    }

    const destination = loadCommunityDestination(activeCommunityId);
    if (!destination || destination.kind === "home") {
      return;
    }

    const channelIsAvailable = sidebarChannels.some(
      (channel) => channel.id === destination.channelId,
    );
    if (!channelIsAvailable) {
      saveCommunityDestination(activeCommunityId, { kind: "home" });
      void goHome({ replace: true });
      return;
    }

    // The normal switch path writes the remembered channel into the hash before
    // the target community mounts, so no intermediate Inbox frame is painted.
    // Older transition callers may still arrive at neutral Home; repair those.
    if (selectedView === "home") {
      void goChannel(destination.channelId, { replace: true });
    }
  }, [
    channelsQuery.dataUpdatedAt,
    channelsQuery.isSuccess,
    communitiesHook.activeCommunity?.id,
    goChannel,
    goHome,
    isSecondaryWindow,
    selectedView,
    sidebarChannels,
  ]);
  const [terminalContextOverride, setTerminalContextOverride] =
    React.useState<TerminalContextOverride | null>(null);
  const { activeChannel, terminalContext } = useTerminalContext({
    channelId: selectedChannelId,
    channels,
    locationSearch: location.search,
    pubkey: identityQuery.data?.pubkey,
    relayUrl: communitiesHook.activeCommunity?.relayUrl,
  });
  const effectiveTerminalContext = terminalContextOverride
    ? {
        ...terminalContext,
        channelId: terminalContextOverride.channelId,
        channelName: terminalContextOverride.channelName,
        threadId: null,
      }
    : terminalContext;
  const managedChannel = React.useMemo(() => {
    const targetChannelId = managedChannelId ?? selectedChannelId;
    return targetChannelId
      ? (channels.find((channel) => channel.id === targetChannelId) ?? null)
      : null;
  }, [channels, managedChannelId, selectedChannelId]);
  const {
    handleChannelNotification,
    handleDmNotification,
    handleThreadReplyDesktopNotification,
  } = useAppShellDesktopNotifications({
    channels,
    enabled: ownsAppGlobals,
    goChannel,
    goHome,
    notificationSettings: notificationSettings.settings,
    openSearchHit,
    pubkey: identityQuery.data?.pubkey,
    silentChannelIds: huddleBackingChannelIds,
  });
  const {
    followedRootIds,
    isFollowing: isFollowingThread,
    followThread,
    unfollowThread,
  } = useThreadFollows(identityQuery.data?.pubkey);
  const {
    markAllChannelsRead: markAllChannelReadMarkers,
    markChannelRead,
    markChannelUnread,
    clearChannelUnreadSource,
    unreadChannelIds,
    topLevelUnreadChannelIds,
    unreadChannelCounts,
    highPriorityUnreadChannelIds,
    unreadChannelNotificationCount,
    getEffectiveTimestamp: getChannelReadAt,
    getOwnTimestamp: getOwnReadAt,
    readStateVersion,
    setContextParentResolver,
    participatedRootIds,
    authoredRootIds,
    recordThreadInteraction,
    threadActivityItems,
    mutedRootIds,
    muteThread,
    unmuteThread,
  } = useUnreadChannels(
    isAuxWindow ? EMPTY_CHANNELS : sidebarChannels,
    isAuxWindow ? null : activeChannel,
    {
      pubkey: identityQuery.data?.pubkey,
      relayClient,
      // Without a relay scope the observed-unread store never opens, so the
      // main window stays its single writer. Read markers still publish.
      relayUrl: isPopout
        ? undefined
        : communitiesHook.activeCommunity?.relayUrl,
      currentPubkey: identityQuery.data?.pubkey,
      mutedChannelIds,
      notifyForActiveChannel: notificationSettings.settings.notifyWhileViewing,
      onChannelMessage: handleChannelNotification,
      onDmMessage: handleDmNotification,
      onLiveMention: refetchHomeFeedFromLiveSignal,
      onThreadReplyDesktopNotification: handleThreadReplyDesktopNotification,
      followedRootIds,
    },
  );

  const {
    getThreadReadAt,
    markThreadRead,
    getMessageReadAt,
    getChannelActivityItemReadAt,
    markMessageRead,
    threadActivityFeedItems,
    locallyUnreadFeedItems,
    unreadThreadFeedItems,
    unreadThreadChannelIds,
  } = useChannelActivityProjection({
    channels,
    feed: homeFeedQuery.data?.feed,
    unreadFeedItemIds: feedItemState.unreadSet,
    getChannelReadAt,
    getOwnReadAt,
    markChannelRead,
    readStateVersion,
    threadActivityItems,
    mutedRootIds,
  });
  const markAllChannelsRead = React.useCallback(() => {
    markAllReadSources({
      activeChannelId: activeChannel?.id ?? null,
      channelActivityItems: unreadThreadFeedItems,
      markAllChannelReadMarkers,
      markActiveChannelRead: (channelId, createdAt) =>
        markChannelRead(channelId, new Date(createdAt * 1_000).toISOString()),
      undoUnreadFeedItem: feedItemState.undoUnread,
      unreadFeedItemIds: feedItemState.unreadSet,
    });
  }, [
    activeChannel?.id,
    feedItemState.undoUnread,
    feedItemState.unreadSet,
    markAllChannelReadMarkers,
    markChannelRead,
    unreadThreadFeedItems,
  ]);

  const { homeBadgeCount, homeBadgeCountExcludingHighPriority } =
    useHomeFeedNotificationState(
      homeFeedQuery.data,
      identityQuery.data?.pubkey,
      notificationSettings.settings,
      notificationSettings.setDesktopEnabled,
      ownsAppGlobals,
      selectedView === "home" && !settingsOpen,
      getChannelReadAt,
      readStateVersion,
      highPriorityUnreadChannelIds,
      feedProfilesQuery.data?.profiles,
      mutedChannelIds,
      feedItemState.unreadSet,
      threadActivityFeedItems,
      getThreadReadAt,
      getMessageReadAt,
      channels,
      huddleBackingChannelIds,
    );
  const dueReminderBadge = useDueReminderBadgeCount(
    identityQuery.data?.pubkey,
    notificationSettings.settings.homeBadgeEnabled,
  );
  const isNotifiedForThread = React.useCallback(
    (rootId: string) =>
      !mutedRootIds.has(rootId) &&
      (followedRootIds.has(rootId) ||
        participatedRootIds.has(rootId) ||
        authoredRootIds.has(rootId)),
    [followedRootIds, mutedRootIds, participatedRootIds, authoredRootIds],
  );

  const handleFollowThread = React.useCallback(
    (rootId: string) => {
      followThread(rootId);
      unmuteThread(rootId);
    },
    [followThread, unmuteThread],
  );

  const handleUnfollowThread = React.useCallback(
    (rootId: string) => {
      unfollowThread(rootId);
      muteThread(rootId);
    },
    [unfollowThread, muteThread],
  );

  const createChannelMutation = useCreateChannelMutation(),
    createForumMutation = useCreateChannelMutation();
  const { applyCanvas, applyAgents } = useApplyTemplate();
  const openDmMutation = useOpenDmMutation();
  const hideDmMutation = useHideDmMutation();
  useDmResurfaceFromMessages({
    pubkey: isPopout ? undefined : identityQuery.data?.pubkey,
    relayUrl: communitiesHook.activeCommunity?.relayUrl,
    reopen: openDmMutation.mutateAsync,
  });
  const {
    browseDialogType,
    openBrowseChannels: handleOpenBrowseChannels,
    onBrowseDialogOpenChange: handleBrowseDialogOpenChange,
    getCreateSuccess,
  } = useChannelBrowserDialog(() => void refetchChannels());
  const handleOpenSearch = React.useCallback(() => {
    setSearchFocusRequest((request) => request + 1);
    void refetchChannels();
  }, [refetchChannels]);
  const handleOpenChannelSearch = React.useCallback(() => {
    setScopeSearchFocusRequest((request) => request + 1);
    void refetchChannels();
  }, [refetchChannels]);

  const handleBrowseChannelJoin = React.useCallback(
    async (channelId: string) => {
      await joinChannel(channelId);
      await queryClient.invalidateQueries({ queryKey: channelsQueryKey });
    },
    [queryClient],
  );

  const handleCreateChannel = React.useCallback(
    async (
      {
        description,
        name,
        visibility,
        ttlSeconds,
        templateId,
      }: {
        name: string;
        description?: string;
        visibility: ChannelVisibility;
        ttlSeconds?: number;
        templateId?: string;
      },
      onCreated?: (channelId: string) => void,
    ) => {
      const createdChannel = await createChannelMutation.mutateAsync({
        name,
        description,
        channelType: "stream",
        visibility,
        ttlSeconds,
      });

      await applyCanvas(templateId, createdChannel.id, name);
      await goChannel(createdChannel.id);
      onCreated?.(createdChannel.id);
      void applyAgents(templateId, createdChannel.id);
    },
    [applyAgents, applyCanvas, createChannelMutation, goChannel],
  );
  const handleCreateForum = React.useCallback(
    async ({
      description,
      name,
      visibility,
      ttlSeconds,
      templateId,
    }: {
      name: string;
      description?: string;
      visibility: ChannelVisibility;
      ttlSeconds?: number;
      templateId?: string;
    }) => {
      const createdForum = await createForumMutation.mutateAsync({
        name,
        description,
        channelType: "forum",
        visibility,
        ttlSeconds,
      });

      await applyCanvas(templateId, createdForum.id, name);
      await goChannel(createdForum.id);
      void applyAgents(templateId, createdForum.id);
    },
    [applyAgents, applyCanvas, createForumMutation, goChannel],
  );

  // The channel browser can create either a stream or a forum depending on
  // which section opened it. Route to the matching handler.
  const handleBrowseChannelCreate = React.useCallback(
    async (input: {
      name: string;
      description?: string;
      visibility: ChannelVisibility;
      ttlSeconds?: number;
      templateId?: string;
    }) => {
      if (browseDialogType === "forum") {
        await handleCreateForum(input);
      } else {
        await handleCreateChannel(input, getCreateSuccess() ?? undefined);
      }
    },
    [
      browseDialogType,
      handleCreateChannel,
      handleCreateForum,
      getCreateSuccess,
    ],
  );

  const handleHideDm = React.useCallback(
    async (channelId: string) => {
      try {
        await hideDmMutation.mutateAsync(channelId);
      } catch {
        return;
      }

      if (selectedChannelId === channelId) {
        void goHome();
      }
    },
    [goHome, hideDmMutation, selectedChannelId],
  );
  const handleOpenSettings = React.useCallback(
    (section: SettingsSection = DEFAULT_SETTINGS_SECTION) => {
      setIsChannelManagementOpen(false);
      // Settings belong to the main window; a pop-out or community window
      // just brings it forward.
      if (isSecondaryWindow) {
        openInMainWindow(null, true);
        return;
      }
      void goSettings(section);
    },
    [goSettings, isSecondaryWindow],
  );
  const handleCloseSettings = React.useCallback(
    () => closeSettings(),
    [closeSettings],
  );
  // Section switches rewrite the settings entry rather than stacking one
  // history entry per section, so back always exits settings in one step.
  const handleSettingsSectionChange = React.useCallback(
    (section: SettingsSection) => {
      void goSettings(section, { replace: true });
    },
    [goSettings],
  );

  const handleOpenSearchResult = React.useCallback(
    (hit: SearchHit, query: string) => {
      void openSearchHit(hit, { query });
    },
    [openSearchHit],
  );
  const openDmAsync = openDmMutation.mutateAsync;
  const handleOpenSearchResultInNewWindow = React.useCallback(
    (result: SearchResult) => {
      void openSearchResultInNewWindow(result, {
        openDm: openDmAsync,
        openInNewWindow,
      }).catch((error: unknown) => {
        console.error("Failed to open search result in a new window:", error);
      });
    },
    [openDmAsync, openInNewWindow],
  );
  useAppShellLifecycleEffects({
    desktopBadgeEnabled: ownsAppGlobals,
    homeBadgeCountExcludingHighPriority,
    topLevelUnreadChannelIds,
    unreadChannelNotificationCount,
  });
  // Dispatch `buzz://` deep links only from the main window; the companion is dedicated to its active Huddle route.
  useAppDeepLinks(ownsAppGlobals);
  useMainWindowPopoutNavigation(ownsAppGlobals);
  const handleOpenCreateChannel = React.useCallback(
    () => setIsCreateChannelOpen(true),
    [],
  );
  useAppShellKeyboardShortcuts({
    activeChannelId: selectedView === "channel" ? selectedChannelId : null,
    canSearchCurrentChannel:
      selectedView === "channel" && Boolean(activeChannel),
    disabled: settingsOpen || isAuxWindow,
    onBrowseChannels: handleOpenBrowseChannels,
    onCreateChannel: handleOpenCreateChannel,
    onGoHome: goHome,
    onNewMessage: goNewMessage,
    onSearchCurrentChannel: handleOpenChannelSearch,
    onSearchEverything: handleOpenSearch,
  });
  useSettingsShortcuts({
    onClose: handleCloseSettings,
    onOpenSettings: handleOpenSettings,
    open: ownsAppGlobals ? settingsOpen : undefined,
  });
  useMarkAsReadShortcuts({
    // Escape still marks the visible channel read in a pop-out; Shift+Escape
    // (mark everything read) is a main-window action.
    allowMarkAll: ownsAppGlobals,
    activeChannelId: activeChannel?.id ?? null,
    activeChannelLastMessageAt: activeChannel?.lastMessageAt,
    markAllChannelsRead,
    markChannelRead,
    selectedView,
  });
  return (
    <PreventSleepProvider>
      {ownsAppGlobals ? (
        <AppShellTrayMenu
          channels={channels}
          goChannel={goChannel}
          openCreateChannel={handleOpenCreateChannel}
        />
      ) : null}
      <ChannelNavigationProvider channels={channels}>
        <AppShellProvider
          value={{
            markAllChannelsRead,
            markChannelRead,
            markChannelUnread,
            clearChannelUnreadSource,
            openBrowseChannels: handleOpenBrowseChannels,
            openCreateChannel: handleOpenCreateChannel,
            openChannelManagement: (channelId?: string) => {
              setManagedChannelId(
                typeof channelId === "string" ? channelId : null,
              );
              setIsChannelManagementOpen(true);
            },
            getChannelReadAt,
            getThreadReadAt,
            markThreadRead,
            getMessageReadAt,
            getChannelActivityItemReadAt,
            markMessageRead,
            readStateVersion,
            setContextParentResolver,
            followThread: handleFollowThread,
            unfollowThread: handleUnfollowThread,
            isFollowingThread,
            isNotifiedForThread,
            recordThreadInteraction,
            isThreadMuted: (rootId) => mutedRootIds.has(rootId),
            threadActivityItems,
            threadActivityFeedItems,
            locallyUnreadFeedItems,
            unreadThreadFeedItems,
            unreadThreadChannelIds,
            topLevelUnreadChannelIds,
            hasSidebarUnreadProjections: true,
            feedItemState,
            onOpenSettings: handleOpenSettings,
          }}
        >
          <AppHuddleShell
            currentPubkey={identityQuery.data?.pubkey}
            isCompanionOpen={isHuddleCompanionOpen}
            isDrawerOpen={isHuddleDrawerOpen}
            isPopout={isSecondaryWindow}
            isRoom={isHuddleRoom}
            onCompanionOpen={handleHuddleCompanionOpen}
            onHuddleStartPendingChange={handleHuddleStartPendingChange}
            onHuddleStarted={handleHuddleStarted}
            onShowHuddleInMainApp={showHuddleInMainApp}
            onViewHuddleChannel={viewHuddleChannel}
            onVisibilityChange={handleHuddleVisibilityChange}
          >
            {hasCommunityRail && ownsAppGlobals ? (
              <CommunityRail
                activeCommunityId={communitiesHook.activeCommunity?.id ?? null}
                onAddCommunity={addCommunityDialog.openDialog}
                onReorderCommunities={communitiesHook.reorderCommunities}
                onSwitchCommunity={handleSwitchCommunity}
                onUpdateCommunity={communitiesHook.updateCommunity}
                communities={communitiesHook.communities}
              />
            ) : null}
            <SidebarProvider
              className="relative z-10 min-h-0 min-w-0 flex-1 flex-col overflow-visible"
              data-testid="app-sidebar-layer"
            >
              <AppProfilePanelProvider>
                <AppWorkflowEditorOverlayProvider>
                  {isPopout ? (
                    <PopoutHeader
                      channels={channels}
                      currentPubkey={identityQuery.data?.pubkey}
                    />
                  ) : null}
                  {!settingsOpen && !isAuxWindow ? (
                    <AppTopChrome
                      canGoBack={canGoBack}
                      canGoForward={canGoForward}
                      communityWindowName={
                        isCommunityWindowShell
                          ? (communitiesHook.activeCommunity?.name ?? null)
                          : undefined
                      }
                      hasCommunityRail={hasCommunityRail && ownsAppGlobals}
                      onGoBack={goBack}
                      onGoForward={goForward}
                    />
                  ) : null}
                  {settingsOpen ? (
                    <div className="flex min-h-0 flex-1 overflow-hidden">
                      <React.Suspense fallback={null}>
                        <LazySettingsScreen
                          currentPubkey={identityQuery.data?.pubkey}
                          fallbackDisplayName={identityQuery.data?.displayName}
                          isUpdatingDesktopNotifications={
                            notificationSettings.isUpdatingDesktopEnabled
                          }
                          notificationErrorMessage={
                            notificationSettings.errorMessage
                          }
                          notificationPermission={
                            notificationSettings.permission
                          }
                          notificationSettings={notificationSettings.settings}
                          onClose={handleCloseSettings}
                          onSectionChange={handleSettingsSectionChange}
                          onSetDesktopNotificationsEnabled={
                            notificationSettings.setDesktopEnabled
                          }
                          onSetHomeBadgeEnabled={
                            notificationSettings.setHomeBadgeEnabled
                          }
                          onSetSlotAlertsEnabled={
                            notificationSettings.setSlotAlertsEnabled
                          }
                          onSetNotifyWhileViewing={
                            notificationSettings.setNotifyWhileViewing
                          }
                          onSetAllSlotAlertsEnabled={
                            notificationSettings.setAllSlotAlertsEnabled
                          }
                          onSetSoundForSlot={
                            notificationSettings.setSoundForSlot
                          }
                          section={settingsSection}
                        />
                      </React.Suspense>
                    </div>
                  ) : (
                    <div className="relative flex min-h-0 flex-1 overflow-visible">
                      {!isAuxWindow ? (
                        <AppSidebar
                          activeCommunity={communitiesHook.activeCommunity}
                          channels={sidebarChannels}
                          currentPubkey={identityQuery.data?.pubkey}
                          errorMessage={channelsErrorMessage}
                          fallbackDisplayName={identityQuery.data?.displayName}
                          homeBadgeCount={homeBadgeCount + dueReminderBadge}
                          addCommunityPrefill={addCommunityDialog.prefill}
                          isAddCommunityOpen={addCommunityDialog.open}
                          relayConnectionCard={relayConnectionCard}
                          isCreatingChannel={createChannelMutation.isPending}
                          isCreatingForum={createForumMutation.isPending}
                          isLoading={channelsQuery.isLoading}
                          isCreateChannelOpen={isCreateChannelOpen}
                          isHuddleCompanionOpen={isHuddleCompanionOpen}
                          isPresencePending={presenceSession.isPending}
                          onAddCommunity={(community) => {
                            const id = communitiesHook.addCommunity({
                              ...community,
                              pubkey:
                                community.pubkey ?? identityQuery.data?.pubkey,
                            });
                            handleSwitchCommunity(id);
                          }}
                          onAddCommunityOpenChange={
                            addCommunityDialog.onOpenChange
                          }
                          onNewMessage={goNewMessage}
                          onBackgroundClick={requestFocusedThreadClose}
                          onCreateChannelOpenChange={setIsCreateChannelOpen}
                          onOpenAddCommunity={addCommunityDialog.openDialog}
                          onSendFeedback={() => setIsSendFeedbackOpen(true)}
                          onUpdateCommunity={communitiesHook.updateCommunity}
                          onLeaveCommunity={handleLeaveCommunity}
                          onRemoveCommunityFromDevice={removeFromDevice}
                          onSwitchCommunity={handleSwitchCommunity}
                          onCreateAgent={() => requestOpenCreateAgent()}
                          selfPresenceStatus={presenceSession.currentStatus}
                          communities={communitiesHook.communities}
                          onCreateChannel={handleCreateChannel}
                          onCreateForum={handleCreateForum}
                          onHideDm={handleHideDm}
                          onHuddleEnded={handleHuddleEnded}
                          onMarkAllChannelsRead={markAllChannelsRead}
                          onMarkChannelRead={markChannelRead}
                          onMarkChannelUnread={markChannelUnread}
                          onBrowseChannels={handleOpenBrowseChannels}
                          onOpenDm={async ({ pubkeys }) => {
                            const directMessage =
                              await openDmMutation.mutateAsync({
                                pubkeys,
                              });
                            await goChannel(directMessage.id);
                          }}
                          onSelectAgents={() => void goAgents()}
                          onSelectChannel={handleSidebarChannelSelect}
                          onOpenSearchResult={handleOpenSearchResult}
                          onOpenSearchResultInNewWindow={
                            handleOpenSearchResultInNewWindow
                          }
                          searchChannels={channels}
                          searchFocusRequests={[
                            searchFocusRequest,
                            scopeSearchFocusRequest,
                          ]}
                          onSelectHome={() => void goHome()}
                          onSelectProjects={() => void goProjects()}
                          onSelectPulse={() => void goPulse()}
                          onSelectSettings={handleOpenSettings}
                          onSelectWorkflows={() => void goWorkflows()}
                          onSetPresenceStatus={(status) =>
                            presenceSession.setStatus(status)
                          }
                          onSetUserStatus={setUserStatusMutation.mutate}
                          onClearUserStatus={() =>
                            setUserStatusMutation.mutate({
                              text: "",
                              emoji: "",
                            })
                          }
                          profile={profileQuery.data}
                          projectsOverviewActive={
                            location.pathname === "/projects"
                          }
                          selfUserStatus={
                            deferredPubkey
                              ? (visibleUserStatus(
                                  selfStatusQuery.data?.[
                                    deferredPubkey.toLowerCase()
                                  ],
                                ) ?? undefined)
                              : undefined
                          }
                          selectedChannelId={selectedChannelId}
                          selectedView={selectedView}
                          unreadChannelIds={unreadChannelIds}
                          {...{ highPriorityUnreadChannelIds }}
                          previewActivityChannelIds={unreadThreadChannelIds}
                          unreadChannelCounts={unreadChannelCounts}
                          mutedChannelIds={mutedChannelIds}
                          onMuteChannel={muteChannel}
                          onUnmuteChannel={unmuteChannel}
                          starredChannelIds={starredChannelIds}
                          onStarChannel={starChannel}
                          onUnstarChannel={unstarChannel}
                        />
                      ) : null}
                      <TerminalContextOverrideProvider
                        onChange={setTerminalContextOverride}
                      >
                        <AppShellChannelSurface
                          hasCommunityRail={hasCommunityRail}
                          isHuddleRoom={isHuddleRoom}
                          isHuddleRoomStarting={isHuddleRoomStarting}
                          isPopout={isPopout}
                          mainInsetRef={mainInsetRef}
                          terminal={
                            <TerminalBootstrap {...effectiveTerminalContext} />
                          }
                        >
                          {isPopout && location.pathname === "/" ? (
                            <PopoutUnavailableState kind="empty" />
                          ) : isCommunityWindowShell &&
                            isMainWindowOnlyPath(location.pathname) ? (
                            <MainWindowOnlyState />
                          ) : (
                            <Outlet />
                          )}
                        </AppShellChannelSurface>
                      </TerminalContextOverrideProvider>
                      {!isAuxWindow ? (
                        <RelayConnectionOverlay
                          card={relayConnectionCard}
                          errorMessage={channelsErrorMessage}
                          hasCommunityRail={hasCommunityRail}
                          isHuddleDrawerOpen={isHuddleDrawerOpen}
                        />
                      ) : null}
                    </div>
                  )}
                  {isCommunityWindowShell ? null : (
                    <>
                      <RequestedAgentCreateDialogs />
                      <AgentManagementDialogs />
                    </>
                  )}
                  <AppShellOverlays
                    activeChannel={managedChannel}
                    browseDialogType={browseDialogType}
                    channels={channels}
                    currentPubkey={identityQuery.data?.pubkey}
                    isChannelManagementOpen={isChannelManagementOpen}
                    isCreatingBrowseChannel={
                      createChannelMutation.isPending ||
                      createForumMutation.isPending
                    }
                    onBrowseChannelJoin={handleBrowseChannelJoin}
                    onBrowseChannelCreate={handleBrowseChannelCreate}
                    onBrowseDialogOpenChange={handleBrowseDialogOpenChange}
                    onChannelManagementOpenChange={(open) => {
                      setIsChannelManagementOpen(open);
                      if (!open) {
                        setManagedChannelId(null);
                      }
                    }}
                    onDeleteActiveChannel={() => {
                      setIsChannelManagementOpen(false);
                      setManagedChannelId(null);
                      void goHome({ replace: true });
                    }}
                    onSelectChannel={(channelId) => {
                      void goChannel(channelId);
                    }}
                    relayUrl={communitiesHook.activeCommunity?.relayUrl}
                  />
                  <SendFeedbackController
                    onOpenChange={setIsSendFeedbackOpen}
                    open={isSendFeedbackOpen}
                  />
                  {!isAuxWindow ? <ProtectedGlobalOverlay /> : null}
                </AppWorkflowEditorOverlayProvider>
              </AppProfilePanelProvider>
            </SidebarProvider>
          </AppHuddleShell>
        </AppShellProvider>
      </ChannelNavigationProvider>
    </PreventSleepProvider>
  );
}
