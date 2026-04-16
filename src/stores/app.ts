import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Memory,
  Topic,
  SearchHit,
  BootstrapStatus,
  McpStatus,
  HookStatus,
  SetupResult,
  OrganizerReport,
  OrganizerProgress,
} from "@/types";
import { useTauri } from "@/composables/useTauri";

export const useAppStore = defineStore("app", () => {
  const tauri = useTauri();

  // Status
  const bootstrap = ref<BootstrapStatus | null>(null);
  const mcpStatus = ref<McpStatus | null>(null);
  const hookStatus = ref<HookStatus | null>(null);

  // Data
  const topics = ref<Topic[]>([]);
  const memories = ref<Memory[]>([]); // flat list for current topic view
  const searchResults = ref<SearchHit[]>([]);
  const searchQuery = ref("");
  const lastSetupReport = ref<SetupResult | null>(null);

  // UI state
  const loading = ref(false);
  const searching = ref(false);
  const settingUp = ref(false);
  const organizing = ref(false);
  const organizeProgress = ref<OrganizerProgress | null>(null);
  const autoOrganize = ref(false);
  const lastOrganizeReport = ref<OrganizerReport | null>(null);
  const error = ref<string | null>(null);

  // Polling for external changes (e.g. MCP server adding memories)
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let progressUnlisten: UnlistenFn | null = null;
  const lastKnownCount = ref(0);

  // Computed
  const totalMemories = computed(() => bootstrap.value?.memory_count ?? 0);
  const needsSetup = computed(
    () =>
      !bootstrap.value?.managed_section_present ||
      (bootstrap.value?.memory_count ?? 0) === 0,
  );
  const needsMcpRegistration = computed(
    () => !(mcpStatus.value?.registered ?? false),
  );

  // Actions
  async function loadStatus() {
    try {
      const [b, m, h] = await Promise.all([
        tauri.getBootstrapStatus(),
        tauri.getMcpServerStatus(),
        tauri.getHookStatus(),
      ]);
      bootstrap.value = b;
      mcpStatus.value = m;
      hookStatus.value = h;
    } catch (e) {
      error.value = `Status check failed: ${e}`;
    }
  }

  async function enableHook() {
    try {
      await tauri.installHook();
      await loadStatus();
    } catch (e) {
      error.value = String(e);
    }
  }

  async function disableHook() {
    try {
      await tauri.uninstallHook();
      await loadStatus();
    } catch (e) {
      error.value = String(e);
    }
  }

  async function loadTopics() {
    loading.value = true;
    try {
      topics.value = await tauri.listTopics();
    } catch (e) {
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  }

  async function loadMemoriesByTopic(topic: string) {
    loading.value = true;
    try {
      memories.value = await tauri.listMemoriesByTopic(topic);
    } catch (e) {
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  }

  async function loadAllMemories() {
    loading.value = true;
    try {
      memories.value = await tauri.listMemories();
    } catch (e) {
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  }

  async function search(query: string) {
    searchQuery.value = query;
    if (!query.trim()) {
      searchResults.value = [];
      return;
    }
    searching.value = true;
    try {
      searchResults.value = await tauri.searchMemories(query.trim(), 20);
    } catch (e) {
      error.value = String(e);
      searchResults.value = [];
    } finally {
      searching.value = false;
    }
  }

  async function runSetup() {
    settingUp.value = true;
    error.value = null;
    try {
      lastSetupReport.value = await tauri.runFirstTimeSetup();
      await loadStatus();
      await loadTopics();
      // Kick off initial organize pass in background if we have memories
      if (totalMemories.value > 0 && autoOrganize.value) {
        runOrganize(); // fire and forget
      }
    } catch (e) {
      error.value = String(e);
      throw e;
    } finally {
      settingUp.value = false;
    }
  }

  async function registerMcp() {
    try {
      await tauri.registerMcpServer();
      await loadStatus();
    } catch (e) {
      error.value = String(e);
      throw e;
    }
  }

  async function unregisterMcp() {
    try {
      await tauri.unregisterMcpServer();
      await loadStatus();
    } catch (e) {
      error.value = String(e);
      throw e;
    }
  }

  async function runOrganize() {
    if (organizing.value) return;
    organizing.value = true;
    organizeProgress.value = {
      phase: "starting",
      message: "Starting organizer",
      current: 0,
      total: 0,
    };
    error.value = null;
    try {
      lastOrganizeReport.value = await tauri.runOrganizePass();
      await loadStatus();
      await loadTopics();
    } catch (e) {
      error.value = String(e);
    } finally {
      organizing.value = false;
      organizeProgress.value = null;
    }
  }

  async function undoLast() {
    try {
      await tauri.undoLastOrganize();
      await loadStatus();
      await loadTopics();
    } catch (e) {
      error.value = String(e);
      throw e;
    }
  }

  async function loadAutoOrganize() {
    try {
      autoOrganize.value = await tauri.getAutoOrganize();
    } catch (e) {
      console.error("load auto-organize setting:", e);
    }
  }

  async function setAutoOrganizeEnabled(enabled: boolean) {
    autoOrganize.value = enabled;
    try {
      await tauri.setAutoOrganize(enabled);
    } catch (e) {
      error.value = String(e);
    }
  }

  async function checkForChanges() {
    try {
      const count = await tauri.memoryCount();
      if (count !== lastKnownCount.value) {
        lastKnownCount.value = count;
        // Refresh bootstrap status (updates totalMemories)
        const b = await tauri.getBootstrapStatus();
        bootstrap.value = b;
        // Refresh topics list
        await loadTopics();
      }
    } catch {
      // Silently ignore polling errors
    }
  }

  function startPolling() {
    stopPolling();
    pollTimer = setInterval(checkForChanges, 5000);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
    if (progressUnlisten) {
      progressUnlisten();
      progressUnlisten = null;
    }
  }

  async function startProgressListener() {
    if (progressUnlisten) return;
    progressUnlisten = await listen<OrganizerProgress>(
      "organizer:progress",
      (event) => {
        organizeProgress.value = event.payload;
      },
    );
  }

  async function initialize() {
    await loadStatus();
    await loadAutoOrganize();
    await startProgressListener();
    if (!needsSetup.value) {
      lastKnownCount.value = totalMemories.value;
      await loadTopics();
      startPolling();
    }
  }

  return {
    // state
    bootstrap,
    mcpStatus,
    hookStatus,
    topics,
    memories,
    searchResults,
    searchQuery,
    lastSetupReport,
    lastOrganizeReport,
    autoOrganize,
    loading,
    searching,
    settingUp,
    organizing,
    organizeProgress,
    error,
    // computed
    totalMemories,
    needsSetup,
    needsMcpRegistration,
    // actions
    loadStatus,
    loadTopics,
    loadMemoriesByTopic,
    loadAllMemories,
    search,
    runSetup,
    registerMcp,
    unregisterMcp,
    runOrganize,
    undoLast,
    loadAutoOrganize,
    setAutoOrganizeEnabled,
    enableHook,
    disableHook,
    initialize,
    stopPolling,
    lastKnownCount,
  };
});
