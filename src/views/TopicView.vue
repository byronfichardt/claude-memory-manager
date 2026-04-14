<script setup lang="ts">
import { onMounted, ref, watch, computed } from "vue";
import { useRouter } from "vue-router";
import { useAppStore } from "@/stores/app";
import { useTauri } from "@/composables/useTauri";
import MarkdownBody from "@/components/MarkdownBody.vue";
import type { Memory, RelatedMemoryEntry } from "@/types";

const props = defineProps<{ name: string }>();
const router = useRouter();
const app = useAppStore();
const tauri = useTauri();

const expandedId = ref<string | null>(null);
const mode = ref<"preview" | "edit">("preview");
const deleteConfirm = ref<string | null>(null);

// Editing state — only populated when a memory is expanded + in edit mode
const editTitle = ref("");
const editDescription = ref("");
const editContent = ref("");
const editTopic = ref("");
const originalSnapshot = ref<Memory | null>(null);
const saving = ref(false);
const saveError = ref("");
const relatedMap = ref<Record<string, RelatedMemoryEntry[]>>({});

const isUntopiced = computed(() => props.name === "__untopiced__");
const displayTitle = computed(() =>
  isUntopiced.value ? "Unclassified" : props.name,
);

async function load() {
  if (isUntopiced.value) {
    const all = await tauri.listMemories();
    app.memories = all.filter((m: Memory) => !m.topic);
  } else {
    await app.loadMemoriesByTopic(props.name);
  }
}

onMounted(load);
watch(() => props.name, load);

function goBack() {
  router.push({ name: "home" });
}

function toggle(memory: Memory) {
  if (expandedId.value === memory.id) {
    expandedId.value = null;
    mode.value = "preview";
    saveError.value = "";
    return;
  }
  expandedId.value = memory.id;
  mode.value = "preview";
  saveError.value = "";
  originalSnapshot.value = memory;
  editTitle.value = memory.title;
  editDescription.value = memory.description;
  editContent.value = memory.content;
  editTopic.value = memory.topic ?? "";

  // Lazy-load related memories via graph
  if (!relatedMap.value[memory.id]) {
    tauri.getRelatedMemories(memory.id, 1).then((res) => {
      relatedMap.value[memory.id] = res.related;
    }).catch(() => {
      // Graph may be empty — not an error
    });
  }
}

function switchToEdit() {
  mode.value = "edit";
}

function cancelEdit() {
  if (!originalSnapshot.value) return;
  editTitle.value = originalSnapshot.value.title;
  editDescription.value = originalSnapshot.value.description;
  editContent.value = originalSnapshot.value.content;
  editTopic.value = originalSnapshot.value.topic ?? "";
  mode.value = "preview";
  saveError.value = "";
}

const isDirty = computed(() => {
  const snap = originalSnapshot.value;
  if (!snap) return false;
  return (
    editTitle.value !== snap.title ||
    editDescription.value !== snap.description ||
    editContent.value !== snap.content ||
    editTopic.value !== (snap.topic ?? "")
  );
});

async function saveEdits() {
  if (!originalSnapshot.value || !isDirty.value) return;
  saving.value = true;
  saveError.value = "";
  try {
    const updated = await tauri.updateMemory(
      originalSnapshot.value.id,
      editTitle.value.trim(),
      editDescription.value.trim(),
      editContent.value,
      editTopic.value.trim() || undefined,
    );
    originalSnapshot.value = updated;
    // If topic changed, reload the list — this memory may have moved out of view
    if (updated.topic !== props.name && !isUntopiced.value) {
      expandedId.value = null;
      await load();
    } else {
      // Refresh the memory in the current list
      const idx = app.memories.findIndex((m) => m.id === updated.id);
      if (idx >= 0) app.memories[idx] = updated;
    }
    mode.value = "preview";
  } catch (e) {
    saveError.value = String(e);
  } finally {
    saving.value = false;
  }
}

async function confirmDelete(id: string) {
  try {
    await tauri.deleteMemory(id);
    await load();
    deleteConfirm.value = null;
    expandedId.value = null;
  } catch (e) {
    app.error = String(e);
  }
}
</script>

<template>
  <div class="topic">
    <button class="back-btn" @click="goBack">
      <svg viewBox="0 0 16 16" fill="currentColor" class="back-icon">
        <path d="M9.78 12.78a.75.75 0 01-1.06 0L4.47 8.53a.75.75 0 010-1.06l4.25-4.25a.75.75 0 111.06 1.06L6.06 8l3.72 3.72a.75.75 0 010 1.06z" />
      </svg>
      Home
    </button>

    <div class="topic-header">
      <h1 class="topic-title">{{ displayTitle }}</h1>
      <span class="topic-count-meta">{{ app.memories.length }} memor{{ app.memories.length === 1 ? "y" : "ies" }}</span>
    </div>

    <div v-if="app.loading" class="empty">Loading...</div>
    <div v-else-if="app.memories.length === 0" class="empty">No memories in this topic.</div>

    <div v-else class="memory-list">
      <div
        v-for="memory in app.memories"
        :key="memory.id"
        class="memory-item"
        :class="{ 'is-expanded': expandedId === memory.id }"
      >
        <button class="memory-head" @click="toggle(memory)">
          <div class="memory-info">
            <div class="memory-title">{{ memory.title }}</div>
            <div v-if="memory.description" class="memory-desc">{{ memory.description }}</div>
          </div>
          <div class="memory-meta">
            <span v-if="memory.memory_type" class="type-badge" :class="`type-${memory.memory_type}`">
              {{ memory.memory_type }}
            </span>
            <svg class="chevron" :class="{ 'is-open': expandedId === memory.id }" viewBox="0 0 16 16" fill="currentColor">
              <path d="M6.22 4.22a.75.75 0 011.06 0l3.25 3.25a.75.75 0 010 1.06l-3.25 3.25a.75.75 0 01-1.06-1.06L8.94 8 6.22 5.28a.75.75 0 010-1.06z" />
            </svg>
          </div>
        </button>

        <div v-if="expandedId === memory.id" class="memory-body">
          <!-- Mode switcher -->
          <div class="mode-bar">
            <div class="mode-tabs">
              <button
                class="mode-tab"
                :class="{ 'is-active': mode === 'preview' }"
                @click.stop="mode = 'preview'"
              >
                Preview
              </button>
              <button
                class="mode-tab"
                :class="{ 'is-active': mode === 'edit' }"
                @click.stop="switchToEdit"
              >
                Edit
              </button>
            </div>
            <span
              v-if="mode === 'edit' && isDirty"
              class="dirty-indicator"
            >
              Unsaved changes
            </span>
          </div>

          <!-- Preview mode -->
          <div v-if="mode === 'preview'" class="preview-pane">
            <div class="preview-meta">
              <div class="meta-row">
                <span class="meta-label">Title</span>
                <span class="meta-value">{{ memory.title }}</span>
              </div>
              <div v-if="memory.description" class="meta-row">
                <span class="meta-label">Description</span>
                <span class="meta-value">{{ memory.description }}</span>
              </div>
              <div v-if="memory.topic" class="meta-row">
                <span class="meta-label">Topic</span>
                <span class="meta-value">{{ memory.topic }}</span>
              </div>
            </div>
            <MarkdownBody :content="memory.content" />
          </div>

          <!-- Edit mode -->
          <div v-else class="edit-pane" @click.stop>
            <div class="edit-fields">
              <label class="edit-field">
                <span class="edit-label">Title</span>
                <input
                  v-model="editTitle"
                  type="text"
                  class="edit-input"
                />
              </label>
              <label class="edit-field">
                <span class="edit-label">Description</span>
                <input
                  v-model="editDescription"
                  type="text"
                  class="edit-input"
                />
              </label>
              <label class="edit-field">
                <span class="edit-label">Topic</span>
                <input
                  v-model="editTopic"
                  type="text"
                  class="edit-input"
                  placeholder="(none)"
                />
              </label>
              <label class="edit-field">
                <span class="edit-label">Content (markdown)</span>
                <textarea
                  v-model="editContent"
                  class="edit-textarea"
                  rows="16"
                  spellcheck="false"
                ></textarea>
              </label>
            </div>
            <div v-if="saveError" class="save-error">{{ saveError }}</div>
            <div class="edit-actions">
              <button class="ghost-btn" @click.stop="cancelEdit">Cancel</button>
              <button
                class="primary-btn"
                :disabled="!isDirty || saving"
                @click.stop="saveEdits"
              >
                {{ saving ? "Saving..." : "Save" }}
              </button>
            </div>
          </div>

          <!-- Related memories (graph edges) -->
          <div v-if="relatedMap[memory.id]?.length" class="related-section">
            <div class="related-header">Related Memories</div>
            <div class="related-list">
              <div
                v-for="rel in relatedMap[memory.id]"
                :key="rel.edge.id"
                class="related-item"
              >
                <span class="edge-type-badge" :class="`edge-${rel.edge.edge_type}`">
                  {{ rel.edge.edge_type }}
                </span>
                <span class="related-memory-title">{{ rel.memory.title }}</span>
                <span class="edge-weight">{{ Math.round(rel.edge.weight * 100) }}%</span>
              </div>
            </div>
          </div>

          <!-- Footer: source + delete (always visible) -->
          <div class="memory-footer">
            <span class="memory-source" v-if="memory.source">{{ memory.source }}</span>
            <span class="memory-spacer"></span>
            <button
              v-if="deleteConfirm !== memory.id"
              class="danger-btn"
              @click.stop="deleteConfirm = memory.id"
            >
              Delete
            </button>
            <template v-else>
              <span class="confirm-text">Really delete?</span>
              <button class="ghost-btn" @click.stop="deleteConfirm = null">Cancel</button>
              <button class="danger-btn" @click.stop="confirmDelete(memory.id)">Delete</button>
            </template>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.topic {
  max-width: 48rem;
  margin: 0 auto;
  padding: 1.5rem;
}

.back-btn {
  display: inline-flex;
  align-items: center;
  gap: 0.25rem;
  padding: 0.375rem 0.625rem 0.375rem 0.375rem;
  background: none;
  border: none;
  color: var(--color-text-muted);
  font-size: 0.75rem;
  cursor: pointer;
  border-radius: 0.25rem;
  margin-bottom: 1rem;
}
.back-btn:hover {
  background: var(--color-surface-hover);
  color: var(--color-text-primary);
}
.back-icon {
  width: 0.875rem;
  height: 0.875rem;
}

.topic-header {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  margin-bottom: 1.5rem;
  padding-bottom: 0.75rem;
  border-bottom: 1px solid var(--color-border);
}
.topic-title {
  font-size: 1.5rem;
  font-weight: 600;
  color: var(--color-text-primary);
  margin: 0;
  letter-spacing: -0.02em;
  text-transform: capitalize;
}
.topic-count-meta {
  font-size: 0.75rem;
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
}

.empty {
  text-align: center;
  padding: 3rem 1rem;
  color: var(--color-text-muted);
  font-size: 0.8125rem;
}

.memory-list {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.memory-item {
  border: 1px solid var(--color-border);
  background: var(--color-surface-alt);
  border-radius: 0.5rem;
  overflow: hidden;
  transition: border-color 0.15s;
}
.memory-item.is-expanded {
  border-color: var(--color-border-light);
}

.memory-head {
  display: flex;
  align-items: center;
  gap: 1rem;
  width: 100%;
  padding: 0.75rem 1rem;
  background: none;
  border: none;
  cursor: pointer;
  text-align: left;
}
.memory-head:hover {
  background: var(--color-surface-hover);
}

.memory-info {
  flex: 1;
  min-width: 0;
}
.memory-title {
  font-size: 0.8125rem;
  color: var(--color-text-primary);
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.memory-desc {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
  margin-top: 0.125rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.memory-meta {
  display: flex;
  align-items: center;
  gap: 0.5rem;
}

.type-badge {
  font-size: 0.625rem;
  padding: 0.125rem 0.5rem;
  border-radius: 0.75rem;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  font-weight: 500;
}
.type-user {
  color: var(--color-type-user);
  background: color-mix(in srgb, var(--color-type-user) 12%, transparent);
}
.type-feedback {
  color: var(--color-type-feedback);
  background: color-mix(in srgb, var(--color-type-feedback) 12%, transparent);
}
.type-project {
  color: var(--color-type-project);
  background: color-mix(in srgb, var(--color-type-project) 12%, transparent);
}
.type-reference {
  color: var(--color-type-reference);
  background: color-mix(in srgb, var(--color-type-reference) 12%, transparent);
}

.chevron {
  width: 0.875rem;
  height: 0.875rem;
  color: var(--color-text-muted);
  transition: transform 0.15s;
}
.chevron.is-open {
  transform: rotate(90deg);
}

.memory-body {
  padding: 0 1rem 1rem;
  border-top: 1px solid var(--color-border);
}

/* Mode bar */
.mode-bar {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  margin: 0.875rem 0 0.75rem;
}
.mode-tabs {
  display: inline-flex;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 0.375rem;
  padding: 0.1875rem;
  gap: 0.125rem;
}
.mode-tab {
  padding: 0.25rem 0.75rem;
  background: none;
  border: none;
  color: var(--color-text-muted);
  font-size: 0.6875rem;
  cursor: pointer;
  border-radius: 0.25rem;
  font-weight: 500;
  transition: background 0.1s, color 0.1s;
}
.mode-tab:hover {
  color: var(--color-text-secondary);
}
.mode-tab.is-active {
  background: var(--color-surface-active);
  color: var(--color-text-primary);
}
.dirty-indicator {
  font-size: 0.625rem;
  color: var(--color-health-warning);
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

/* Preview pane */
.preview-pane {
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 0.375rem;
  padding: 1rem 1.25rem;
  max-height: 30rem;
  overflow-y: auto;
}
.preview-meta {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  padding-bottom: 0.75rem;
  margin-bottom: 0.75rem;
  border-bottom: 1px solid var(--color-border);
}
.meta-row {
  display: flex;
  gap: 0.5rem;
  font-size: 0.6875rem;
  line-height: 1.5;
}
.meta-label {
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  min-width: 5rem;
  flex-shrink: 0;
}
.meta-value {
  color: var(--color-text-secondary);
  word-break: break-word;
}

/* Edit pane */
.edit-pane {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
}
.edit-fields {
  display: flex;
  flex-direction: column;
  gap: 0.625rem;
}
.edit-field {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}
.edit-label {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
}
.edit-input {
  padding: 0.5rem 0.75rem;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 0.375rem;
  color: var(--color-text-primary);
  font-size: 0.8125rem;
  outline: none;
  transition: border-color 0.15s;
}
.edit-input:focus {
  border-color: var(--color-accent-muted);
}
.edit-textarea {
  padding: 0.75rem;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 0.375rem;
  color: var(--color-text-primary);
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  font-size: 0.75rem;
  line-height: 1.6;
  outline: none;
  resize: vertical;
  min-height: 14rem;
  transition: border-color 0.15s;
}
.edit-textarea:focus {
  border-color: var(--color-accent-muted);
}
.edit-actions {
  display: flex;
  gap: 0.5rem;
  justify-content: flex-end;
}
.save-error {
  padding: 0.5rem 0.75rem;
  border: 1px solid color-mix(in srgb, var(--color-health-error) 30%, transparent);
  background: color-mix(in srgb, var(--color-health-error) 10%, transparent);
  color: var(--color-health-error);
  border-radius: 0.375rem;
  font-size: 0.6875rem;
}

.memory-footer {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  margin-top: 0.875rem;
  padding-top: 0.75rem;
  border-top: 1px solid var(--color-border);
}
.memory-source {
  font-size: 0.625rem;
  color: var(--color-text-muted);
  font-family: ui-monospace, monospace;
}
.memory-spacer {
  flex: 1;
}

.danger-btn,
.ghost-btn,
.primary-btn {
  padding: 0.375rem 0.875rem;
  font-size: 0.6875rem;
  border-radius: 0.25rem;
  border: 1px solid transparent;
  background: none;
  cursor: pointer;
  font-weight: 500;
  transition: background 0.1s, border-color 0.1s, color 0.1s;
}
.primary-btn {
  background: var(--color-accent);
  color: var(--color-surface);
}
.primary-btn:hover:not(:disabled) {
  background: var(--color-accent-hover);
}
.primary-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.danger-btn {
  color: var(--color-health-error);
  border-color: color-mix(in srgb, var(--color-health-error) 30%, transparent);
}
.danger-btn:hover {
  background: color-mix(in srgb, var(--color-health-error) 12%, transparent);
}
.ghost-btn {
  color: var(--color-text-muted);
  border-color: var(--color-border);
}
.ghost-btn:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}
.confirm-text {
  font-size: 0.6875rem;
  color: var(--color-health-warning);
}

/* Related memories section */
.related-section {
  margin-top: 0.875rem;
  padding-top: 0.75rem;
  border-top: 1px solid var(--color-border);
}
.related-header {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin-bottom: 0.5rem;
}
.related-list {
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
}
.related-item {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.375rem 0.5rem;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 0.25rem;
  font-size: 0.75rem;
}
.edge-type-badge {
  font-size: 0.5625rem;
  padding: 0.0625rem 0.375rem;
  border-radius: 0.5rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  font-weight: 500;
  white-space: nowrap;
  color: var(--color-text-muted);
  background: color-mix(in srgb, var(--color-text-muted) 12%, transparent);
}
.edge-relates-to {
  color: var(--color-accent);
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
}
.edge-depends-on {
  color: var(--color-health-warning);
  background: color-mix(in srgb, var(--color-health-warning) 12%, transparent);
}
.edge-supersedes {
  color: var(--color-type-project);
  background: color-mix(in srgb, var(--color-type-project) 12%, transparent);
}
.edge-contradicts {
  color: var(--color-health-error);
  background: color-mix(in srgb, var(--color-health-error) 12%, transparent);
}
.related-memory-title {
  flex: 1;
  min-width: 0;
  color: var(--color-text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.edge-weight {
  font-size: 0.625rem;
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
</style>
