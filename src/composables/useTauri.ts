import { invoke } from "@tauri-apps/api/core";
import type {
  Memory,
  Topic,
  SearchHit,
  BootstrapStatus,
  SetupResult,
  McpStatus,
  ConfigDirRegistration,
  HookStatus,
  ConfigDirHookStatus,
  OrganizerReport,
  HistoryEntry,
} from "@/types";

export function useTauri() {
  return {
    // Bootstrap & setup
    getBootstrapStatus: () => invoke<BootstrapStatus>("get_bootstrap_status"),
    runFirstTimeSetup: () => invoke<SetupResult>("run_first_time_setup"),

    // Memories
    listMemories: () => invoke<Memory[]>("store_list_memories"),
    listMemoriesByTopic: (topic: string) =>
      invoke<Memory[]>("store_list_memories_by_topic", { topic }),
    fetchMemory: (id: string) => invoke<Memory | null>("fetch_memory", { id }),
    searchMemories: (query: string, limit?: number) =>
      invoke<SearchHit[]>("search_memories_fts", { query, limit }),
    addMemory: (
      title: string,
      description: string,
      content: string,
      memoryType?: string,
      topic?: string,
    ) =>
      invoke<Memory>("store_add_memory", {
        title,
        description,
        content,
        memoryType,
        topic,
      }),
    updateMemory: (
      id: string,
      title: string,
      description: string,
      content: string,
      topic?: string,
    ) =>
      invoke<Memory>("store_update_memory", {
        id,
        title,
        description,
        content,
        topic,
      }),
    deleteMemory: (id: string) => invoke<void>("store_delete_memory", { id }),

    // Topics
    listTopics: () => invoke<Topic[]>("list_topics"),

    // MCP registration
    registerMcpServer: () =>
      invoke<ConfigDirRegistration[]>("register_mcp_server"),
    unregisterMcpServer: () =>
      invoke<ConfigDirRegistration[]>("unregister_mcp_server"),
    getMcpServerStatus: () => invoke<McpStatus>("get_mcp_server_status"),

    // Hooks
    getHookStatus: () => invoke<HookStatus>("get_hook_status"),
    installHook: () => invoke<ConfigDirHookStatus[]>("install_hook"),
    uninstallHook: () => invoke<ConfigDirHookStatus[]>("uninstall_hook"),

    // Organizer
    runOrganizePass: () => invoke<OrganizerReport>("run_organize_pass"),
    undoLastOrganize: () => invoke<string>("undo_last_organize"),
    listHistory: (limit?: number) =>
      invoke<HistoryEntry[]>("list_history", { limit }),
    getAutoOrganize: () => invoke<boolean>("get_auto_organize"),
    setAutoOrganize: (enabled: boolean) =>
      invoke<void>("set_auto_organize", { enabled }),
  };
}
