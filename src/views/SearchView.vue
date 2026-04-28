<script setup lang="ts">
import { onMounted, watch, ref, computed } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useAppStore } from "@/stores/app";
import { useTauri } from "@/composables/useTauri";
import MarkdownBody from "@/components/MarkdownBody.vue";
import type { Memory, DateFilter } from "@/types";

const route = useRoute();
const router = useRouter();
const app = useAppStore();
const tauri = useTauri();

const openMemory = ref<Memory | null>(null);
const dateFilter = ref<DateFilter>("all");
const timelineMemories = ref<Memory[]>([]);
const timelineLoading = ref(false);

const DATE_FILTERS: { label: string; value: DateFilter; ms: number | null }[] = [
  { label: "All time", value: "all", ms: null },
  { label: "Last 7 days", value: "7d", ms: 7 * 24 * 60 * 60 * 1000 },
  { label: "Last 30 days", value: "30d", ms: 30 * 24 * 60 * 60 * 1000 },
  { label: "Last 90 days", value: "90d", ms: 90 * 24 * 60 * 60 * 1000 },
];

const showingTimeline = computed(() => dateFilter.value !== "all" && !app.searchQuery);

async function runQuery(q: string) {
  await app.search(q);
}

async function applyDateFilter(filter: DateFilter) {
  dateFilter.value = filter;
  if (filter === "all") {
    timelineMemories.value = [];
    return;
  }
  const entry = DATE_FILTERS.find((f) => f.value === filter);
  if (!entry?.ms) return;
  timelineLoading.value = true;
  try {
    timelineMemories.value = await tauri.listMemoriesSince(Date.now() - entry.ms, 200);
  } catch (e) {
    app.error = String(e);
  } finally {
    timelineLoading.value = false;
  }
}

onMounted(() => {
  const q = route.query.q;
  if (typeof q === "string") runQuery(q);
});

watch(
  () => route.query.q,
  (q) => {
    if (typeof q === "string") runQuery(q);
  },
);

async function openResult(id: string) {
  try {
    const m = await tauri.fetchMemory(id);
    openMemory.value = m;
  } catch (e) {
    app.error = String(e);
  }
}

function closeMemory() {
  openMemory.value = null;
}

function gotoTopic(topic: string | null) {
  if (!topic) return;
  router.push({ name: "topic", params: { name: topic } });
}

function highlight(snippet: string): string {
  return snippet.replace(/\[([^\]]+)\]/g, '<mark>$1</mark>');
}

function formatDate(ts: number): string {
  return new Date(ts * 1000).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}
</script>

<template>
  <div class="search">
    <div class="search-head">
      <h1 class="search-title">
        <template v-if="app.searchQuery">
          Search results <span class="query">for "{{ app.searchQuery }}"</span>
        </template>
        <template v-else>Timeline</template>
      </h1>
      <span v-if="!app.searching && app.searchQuery" class="count">{{ app.searchResults.length }} result{{ app.searchResults.length === 1 ? "" : "s" }}</span>
    </div>

    <!-- Date filter bar (visible when no active search query) -->
    <div v-if="!app.searchQuery" class="date-filters">
      <button
        v-for="f in DATE_FILTERS"
        :key="f.value"
        class="date-filter-btn"
        :class="{ 'is-active': dateFilter === f.value }"
        @click="applyDateFilter(f.value)"
      >
        {{ f.label }}
      </button>
    </div>

    <div v-if="app.searching || timelineLoading" class="empty">Loading...</div>
    <div v-else-if="app.searchResults.length === 0 && app.searchQuery" class="empty">
      No matches for "{{ app.searchQuery }}"
    </div>
    <div v-else-if="showingTimeline && timelineMemories.length === 0" class="empty">
      No memories in this time window.
    </div>
    <div v-else-if="!app.searchQuery && !showingTimeline" class="empty">Type a query in the search bar above, or select a time filter.</div>

    <!-- Timeline view -->
    <div v-if="showingTimeline && timelineMemories.length > 0" class="result-list">
      <button
        v-for="mem in timelineMemories"
        :key="mem.id"
        class="result-item"
        @click="openMemory = mem"
      >
        <div class="result-head">
          <span class="result-title">{{ mem.title }}</span>
          <span v-if="mem.topic" class="topic-tag" @click.stop="gotoTopic(mem.topic)">{{ mem.topic }}</span>
        </div>
        <div v-if="mem.description" class="result-desc">{{ mem.description }}</div>
        <div class="result-date">{{ formatDate(mem.updated_at) }}</div>
      </button>
    </div>

    <!-- Search results -->
    <div v-else-if="app.searchQuery" class="result-list">
      <button
        v-for="hit in app.searchResults"
        :key="hit.id"
        class="result-item"
        @click="openResult(hit.id)"
      >
        <div class="result-head">
          <span class="result-title">{{ hit.title }}</span>
          <span
            v-if="hit.topic"
            class="topic-tag"
            @click.stop="gotoTopic(hit.topic)"
          >
            {{ hit.topic }}
          </span>
        </div>
        <div v-if="hit.description" class="result-desc">{{ hit.description }}</div>
        <div class="result-snippet" v-html="highlight(hit.snippet)"></div>
      </button>
    </div>

    <!-- Memory detail modal -->
    <div v-if="openMemory" class="modal-backdrop" @click.self="closeMemory">
      <div class="modal">
        <div class="modal-head">
          <h2 class="modal-title">{{ openMemory.title }}</h2>
          <button class="close-btn" @click="closeMemory">×</button>
        </div>
        <div v-if="openMemory.description" class="modal-desc">{{ openMemory.description }}</div>
        <div class="modal-tags">
          <span v-if="openMemory.topic" class="topic-tag" @click="gotoTopic(openMemory.topic); closeMemory()">
            {{ openMemory.topic }}
          </span>
          <span v-if="openMemory.memory_type" class="type-badge" :class="`type-${openMemory.memory_type}`">
            {{ openMemory.memory_type }}
          </span>
        </div>
        <div class="modal-content">
          <MarkdownBody :content="openMemory.content" />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.search {
  max-width: 48rem;
  margin: 0 auto;
  padding: 1.5rem;
}

.search-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  padding-bottom: 0.75rem;
  border-bottom: 1px solid var(--color-border);
  margin-bottom: 1rem;
}
.search-title {
  font-size: 1rem;
  font-weight: 500;
  color: var(--color-text-primary);
  margin: 0;
}
.query {
  color: var(--color-text-muted);
  font-weight: 400;
}
.count {
  font-size: 0.75rem;
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
}

.date-filters {
  display: flex;
  gap: 0.375rem;
  margin-bottom: 1rem;
  flex-wrap: wrap;
}
.date-filter-btn {
  padding: 0.3125rem 0.75rem;
  background: var(--color-surface-alt);
  border: 1px solid var(--color-border);
  border-radius: 1rem;
  color: var(--color-text-muted);
  font-size: 0.75rem;
  cursor: pointer;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}
.date-filter-btn:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}
.date-filter-btn.is-active {
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
  border-color: color-mix(in srgb, var(--color-accent) 40%, transparent);
  color: var(--color-accent);
}

.result-date {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
}

.empty {
  text-align: center;
  padding: 3rem 1rem;
  color: var(--color-text-muted);
  font-size: 0.8125rem;
}

.result-list {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.result-item {
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
  padding: 0.875rem 1rem;
  text-align: left;
  background: var(--color-surface-alt);
  border: 1px solid var(--color-border);
  border-radius: 0.5rem;
  cursor: pointer;
}
.result-item:hover {
  background: var(--color-surface-hover);
  border-color: var(--color-border-light);
}
.result-head {
  display: flex;
  align-items: center;
  gap: 0.5rem;
}
.result-title {
  font-size: 0.875rem;
  color: var(--color-text-primary);
  font-weight: 500;
}
.result-desc {
  font-size: 0.75rem;
  color: var(--color-text-secondary);
}
.result-snippet {
  font-size: 0.75rem;
  color: var(--color-text-muted);
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  line-height: 1.5;
}
.result-snippet :deep(mark) {
  background: color-mix(in srgb, var(--color-accent) 30%, transparent);
  color: var(--color-accent-hover);
  padding: 0 0.125rem;
  border-radius: 0.125rem;
}

.topic-tag {
  font-size: 0.625rem;
  color: var(--color-accent);
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
  padding: 0.125rem 0.5rem;
  border-radius: 0.75rem;
  text-transform: capitalize;
  cursor: pointer;
}
.topic-tag:hover {
  background: color-mix(in srgb, var(--color-accent) 20%, transparent);
}

.type-badge {
  font-size: 0.625rem;
  padding: 0.125rem 0.5rem;
  border-radius: 0.75rem;
  text-transform: uppercase;
  letter-spacing: 0.05em;
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

/* Modal */
.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  z-index: 100;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 2rem;
}
.modal {
  background: var(--color-surface-alt);
  border: 1px solid var(--color-border-light);
  border-radius: 0.75rem;
  padding: 1.5rem;
  max-width: 48rem;
  width: 100%;
  max-height: 80vh;
  overflow-y: auto;
}
.modal-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 1rem;
}
.modal-title {
  font-size: 1.125rem;
  font-weight: 600;
  color: var(--color-text-primary);
  margin: 0;
  letter-spacing: -0.01em;
}
.close-btn {
  background: none;
  border: none;
  font-size: 1.5rem;
  color: var(--color-text-muted);
  cursor: pointer;
  line-height: 1;
  padding: 0;
}
.close-btn:hover {
  color: var(--color-text-primary);
}
.modal-desc {
  font-size: 0.8125rem;
  color: var(--color-text-secondary);
  margin-top: 0.5rem;
}
.modal-tags {
  display: flex;
  gap: 0.5rem;
  margin-top: 0.75rem;
}
.modal-content {
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  padding: 1rem 1.25rem;
  border-radius: 0.375rem;
  margin: 1rem 0 0;
}
</style>
