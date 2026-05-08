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
  RelatedMemoriesResponse,
  UninstallReport,
  ExportSummary,
  ImportMode,
  ImportReport,
  RepoGraph,
  RepoEdge,
  ScanProposal,
  EmbeddingStatus,
  DreamProposal,
  DreamReport,
} from "@/types";

export function useTauri() {
  return {
    // Bootstrap & setup
    getBootstrapStatus: () => invoke<BootstrapStatus>("get_bootstrap_status"),
    getStartupErrors: () => invoke<string[]>("get_startup_errors"),
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
    memoryCount: () => invoke<number>("store_memory_count"),
    getRelatedMemories: (id: string, depth?: number) =>
      invoke<RelatedMemoriesResponse>("get_related_memories", { id, depth }),

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
    runOrganizePass: (force?: boolean) => invoke<OrganizerReport>("run_organize_pass", { force: force ?? false }),
    undoLastOrganize: () => invoke<string>("undo_last_organize"),
    listHistory: (limit?: number) =>
      invoke<HistoryEntry[]>("list_history", { limit }),
    getAutoOrganize: () => invoke<boolean>("get_auto_organize"),
    setAutoOrganize: (enabled: boolean) =>
      invoke<void>("set_auto_organize", { enabled }),
    getSplitThreshold: () => invoke<number>("get_split_threshold"),
    setSplitThreshold: (threshold: number) =>
      invoke<void>("set_split_threshold", { threshold }),

    // Uninstall
    uninstallEverything: () => invoke<UninstallReport>("uninstall_everything"),

    // Export / import
    exportMemories: (path: string) =>
      invoke<ExportSummary>("export_memories", { path }),
    importMemories: (path: string, mode: ImportMode) =>
      invoke<ImportReport>("import_memories", { path, mode }),

    // Repo relationship graph
    getRepoGraph: (namespace?: string) => invoke<RepoGraph>("get_repo_graph", { namespace }),
    listRepoNamespaces: () => invoke<string[]>("list_repo_namespaces"),
    addRepoEdge: (sourceRepo: string, targetRepo: string, relationshipType: string, evidence: string, namespace: string) =>
      invoke<RepoEdge>("add_repo_edge", { sourceRepo, targetRepo, relationshipType, evidence, namespace }),
    deleteRepoEdge: (id: number) => invoke<void>("delete_repo_edge", { id }),
    scanReposInDirectory: (path: string) => invoke<ScanProposal[]>("scan_repos_in_directory", { path }),

    // Bulk operations
    bulkDeleteMemories: (ids: string[]) =>
      invoke<number>("bulk_delete_memories", { ids }),

    // Timeline / date-filtered list
    listMemoriesSince: (sinceTsMs: number, limit?: number) =>
      invoke<Memory[]>("list_memories_since", {
        sinceTs: Math.floor(sinceTsMs / 1000),
        limit,
      }),

    // Semantic search (embedding management)
    getEmbeddingStatus: () => invoke<EmbeddingStatus>("get_embedding_status"),
    enableSemanticSearch: () => invoke<void>("enable_semantic_search"),
    disableSemanticSearch: () => invoke<void>("disable_semantic_search"),
    triggerEmbeddingSweep: () => invoke<void>("trigger_embedding_sweep"),

    // Dreaming
    runDreamPass: () => invoke<DreamReport>("run_dream_pass"),
    listDreamProposals: () => invoke<DreamProposal[]>("list_dream_proposals"),
    getDreamProposalCount: () => invoke<number>("get_dream_proposal_count"),
    applyDreamProposal: (id: string) => invoke<void>("apply_dream_proposal", { id }),
    dismissDreamProposal: (id: string) => invoke<void>("dismiss_dream_proposal", { id }),
  };
}
