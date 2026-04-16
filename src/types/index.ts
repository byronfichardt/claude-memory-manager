export type MemoryKind = "user" | "feedback" | "project" | "reference";

export interface Memory {
  id: string;
  title: string;
  description: string;
  content: string;
  memory_type: string | null;
  topic: string | null;
  source: string | null;
  project: string | null;
  created_at: number;
  updated_at: number;
  access_count: number;
}

export interface Topic {
  name: string;
  description: string | null;
  color: string | null;
  created_at: number;
  memory_count: number;
}

export interface SearchHit {
  id: string;
  title: string;
  description: string;
  snippet: string;
  topic: string | null;
  memory_type: string | null;
  project: string | null;
  score: number;
}

export interface ConfigDirStatus {
  path: string;
  label: string;
  claude_md_present: boolean;
  managed_section_present: boolean;
  permissions_granted: boolean;
}

export interface BootstrapStatus {
  config_dirs: ConfigDirStatus[];
  memory_count: number;
  ingestion_done: boolean;
  // back-compat fields
  claude_md_exists: boolean;
  claude_md_path: string;
  managed_section_present: boolean;
}

export interface IngestionReport {
  files_scanned: number;
  memories_imported: number;
  memories_skipped: number;
  errors: string[];
}

export interface ConfigDirRegistration {
  label: string;
  path: string;
  success: boolean;
  error: string | null;
}

export interface SetupResult {
  bootstrap: BootstrapStatus;
  ingestion: IngestionReport;
  mcp_registrations: ConfigDirRegistration[];
}

export interface ConfigDirMcpStatus {
  label: string;
  path: string;
  registered: boolean;
}

export interface McpStatus {
  registered: boolean;
  binary_path: string;
  per_config: ConfigDirMcpStatus[];
}

export interface ConfigDirHookStatus {
  label: string;
  path: string;
  installed: boolean;
}

export interface HookStatus {
  enabled: boolean;
  per_config: ConfigDirHookStatus[];
}

export interface MemoryEdge {
  id: number;
  source_id: string;
  target_id: string;
  edge_type: string;
  weight: number;
  source_origin: string;
  created_at: number;
  updated_at: number;
}

export interface RelatedMemoryEntry {
  edge: MemoryEdge;
  memory: Memory;
}

export interface RelatedMemoriesResponse {
  edges: MemoryEdge[];
  related: RelatedMemoryEntry[];
}

export interface OrganizerReport {
  classified_count: number;
  new_topics_created: string[];
  merged_count: number;
  edges_created: number;
  errors: string[];
}

export interface HistoryEntry {
  id: number;
  action: string;
  timestamp: number;
  snapshot: string;
}
