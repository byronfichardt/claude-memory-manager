<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useTauri } from "@/composables/useTauri";
import type { DreamProposal, DreamReport, DreamProgress } from "@/types";

const tauri = useTauri();

const proposals = ref<DreamProposal[]>([]);
const running = ref(false);
const progress = ref<DreamProgress | null>(null);
const lastReport = ref<DreamReport | null>(null);
const error = ref<string | null>(null);
const expanded = ref<Set<string>>(new Set());

let unlisten: UnlistenFn | null = null;

onMounted(async () => {
  unlisten = await listen<DreamProgress>("dreamer:progress", (e) => {
    progress.value = e.payload;
  });
  await loadProposals();
});

onUnmounted(() => {
  unlisten?.();
});

async function loadProposals() {
  try {
    proposals.value = await tauri.listDreamProposals();
  } catch (e) {
    error.value = String(e);
  }
}

async function runDream() {
  if (running.value) return;
  running.value = true;
  error.value = null;
  progress.value = null;
  try {
    lastReport.value = await tauri.runDreamPass();
    await loadProposals();
  } catch (e) {
    error.value = String(e);
  } finally {
    running.value = false;
    progress.value = null;
  }
}

async function apply(proposal: DreamProposal) {
  try {
    await tauri.applyDreamProposal(proposal.id);
    proposals.value = proposals.value.filter((p) => p.id !== proposal.id);
  } catch (e) {
    error.value = String(e);
  }
}

async function dismiss(proposal: DreamProposal) {
  try {
    await tauri.dismissDreamProposal(proposal.id);
    proposals.value = proposals.value.filter((p) => p.id !== proposal.id);
  } catch (e) {
    error.value = String(e);
  }
}

async function applyAll() {
  for (const p of newProposals.value) {
    await apply(p);
  }
}

async function dismissAll() {
  for (const p of [...newProposals.value, ...staleProposals.value]) {
    await dismiss(p);
  }
}

function toggleExpanded(id: string) {
  if (expanded.value.has(id)) {
    expanded.value.delete(id);
  } else {
    expanded.value.add(id);
  }
}

const newProposals = computed(() =>
  proposals.value.filter((p) => p.proposal_type === "new"),
);
const staleProposals = computed(() =>
  proposals.value.filter((p) => p.proposal_type === "stale"),
);

const memoryTypeLabel: Record<string, string> = {
  feedback: "feedback",
  project: "project",
  user: "user",
  reference: "reference",
};
</script>

<template>
  <div class="dream-view">
    <!-- Header -->
    <div class="dream-header">
      <div class="dream-title-row">
        <div class="dream-title">
          <svg viewBox="0 0 16 16" fill="currentColor" class="dream-icon">
            <path d="M8 1a.5.5 0 01.5.5v1a.5.5 0 01-1 0v-1A.5.5 0 018 1zM4.5 3.086a.5.5 0 01.707 0l.707.707a.5.5 0 01-.707.707l-.707-.707a.5.5 0 010-.707zm7 0a.5.5 0 010 .707l-.707.707a.5.5 0 11-.707-.707l.707-.707a.5.5 0 01.707 0zM8 5a3 3 0 100 6 3 3 0 000-6zm-5 3a.5.5 0 01.5-.5h1a.5.5 0 010 1h-1A.5.5 0 013 8zm9 0a.5.5 0 01.5-.5h1a.5.5 0 010 1h-1A.5.5 0 0112 8zm-7.207 3.793a.5.5 0 010 .707l-.707.707a.5.5 0 01-.707-.707l.707-.707a.5.5 0 01.707 0zm8.414 0a.5.5 0 01.707 0l.707.707a.5.5 0 01-.707.707l-.707-.707a.5.5 0 010-.707zM7.5 13a.5.5 0 011 0v1a.5.5 0 01-1 0v-1z"/>
          </svg>
          <span>Dreaming</span>
        </div>
        <button
          class="dream-btn"
          :class="{ 'is-running': running }"
          :disabled="running"
          @click="runDream"
        >
          <span v-if="running" class="btn-spinner" aria-hidden="true"></span>
          <span>{{ running ? progress?.message ?? "Dreaming…" : "Dream now" }}</span>
        </button>
      </div>

      <p class="dream-description">
        Reviews your recent Claude Code sessions, mines patterns and facts not yet in memory, and flags entries that may be outdated.
      </p>

      <!-- Progress bar -->
      <div v-if="running && progress" class="progress-bar-wrap">
        <div
          class="progress-bar"
          :style="{ width: progress.total > 0 ? `${(progress.current / progress.total) * 100}%` : '30%' }"
        ></div>
      </div>

      <!-- Last run summary -->
      <div v-if="lastReport && !running" class="run-summary">
        <span class="summary-item">
          <span class="summary-num">{{ lastReport.transcripts_reviewed }}</span> sessions reviewed
        </span>
        <span class="summary-dot">·</span>
        <span class="summary-item">
          <span class="summary-num">{{ lastReport.new_proposals }}</span> new proposals
        </span>
        <span class="summary-dot">·</span>
        <span class="summary-item">
          <span class="summary-num">{{ lastReport.stale_flags }}</span> stale flags
        </span>
        <template v-if="lastReport.errors.length > 0">
          <span class="summary-dot">·</span>
          <span class="summary-item summary-err">{{ lastReport.errors.length }} error{{ lastReport.errors.length === 1 ? '' : 's' }}</span>
        </template>
      </div>

      <!-- Error -->
      <div v-if="error" class="dream-error">{{ error }}</div>
    </div>

    <!-- Empty state -->
    <div v-if="!running && proposals.length === 0" class="empty-state">
      <div class="empty-icon">
        <svg viewBox="0 0 16 16" fill="currentColor">
          <path d="M8 1a.5.5 0 01.5.5v1a.5.5 0 01-1 0v-1A.5.5 0 018 1zM4.5 3.086a.5.5 0 01.707 0l.707.707a.5.5 0 01-.707.707l-.707-.707a.5.5 0 010-.707zm7 0a.5.5 0 010 .707l-.707.707a.5.5 0 11-.707-.707l.707-.707a.5.5 0 01.707 0zM8 5a3 3 0 100 6 3 3 0 000-6zm-5 3a.5.5 0 01.5-.5h1a.5.5 0 010 1h-1A.5.5 0 013 8zm9 0a.5.5 0 01.5-.5h1a.5.5 0 010 1h-1A.5.5 0 0112 8zm-7.207 3.793a.5.5 0 010 .707l-.707.707a.5.5 0 01-.707-.707l.707-.707a.5.5 0 01.707 0zm8.414 0a.5.5 0 01.707 0l.707.707a.5.5 0 01-.707.707l-.707-.707a.5.5 0 010-.707zM7.5 13a.5.5 0 011 0v1a.5.5 0 01-1 0v-1z"/>
        </svg>
      </div>
      <p class="empty-title">Nothing pending</p>
      <p class="empty-sub">Run a dream pass to mine your recent sessions for new memories and stale entries.</p>
    </div>

    <!-- Proposals -->
    <div v-else-if="proposals.length > 0" class="proposals-wrap">

      <!-- Bulk actions -->
      <div class="bulk-row">
        <span class="bulk-count">{{ proposals.length }} proposal{{ proposals.length === 1 ? '' : 's' }} pending</span>
        <div class="bulk-actions">
          <button v-if="newProposals.length > 0" class="bulk-btn apply-all" @click="applyAll">
            Apply all new
          </button>
          <button class="bulk-btn dismiss-all" @click="dismissAll">
            Dismiss all
          </button>
        </div>
      </div>

      <!-- New memories section -->
      <section v-if="newProposals.length > 0" class="proposal-section">
        <h2 class="section-title">
          <span class="section-dot new-dot"></span>
          New memories
          <span class="section-count">{{ newProposals.length }}</span>
        </h2>
        <div class="proposal-list">
          <div
            v-for="p in newProposals"
            :key="p.id"
            class="proposal-card"
          >
            <div class="card-header" @click="toggleExpanded(p.id)">
              <div class="card-meta">
                <span class="type-badge" :class="`type-${p.memory_type}`">
                  {{ memoryTypeLabel[p.memory_type] ?? p.memory_type }}
                </span>
                <span class="card-title">{{ p.title }}</span>
              </div>
              <svg
                class="chevron"
                :class="{ 'is-open': expanded.has(p.id) }"
                viewBox="0 0 16 16"
                fill="currentColor"
              >
                <path fill-rule="evenodd" d="M4.22 6.22a.75.75 0 011.06 0L8 8.94l2.72-2.72a.75.75 0 111.06 1.06l-3.25 3.25a.75.75 0 01-1.06 0L4.22 7.28a.75.75 0 010-1.06z" clip-rule="evenodd" />
              </svg>
            </div>

            <div v-if="p.description" class="card-description">{{ p.description }}</div>

            <div v-if="expanded.has(p.id)" class="card-expanded">
              <div v-if="p.content" class="card-content">{{ p.content }}</div>
              <div class="card-reasoning">
                <span class="reasoning-label">Why this was flagged:</span>
                {{ p.reasoning }}
              </div>
            </div>

            <div class="card-actions">
              <button class="action-btn apply-btn" @click="apply(p)">Save to memory</button>
              <button class="action-btn dismiss-btn" @click="dismiss(p)">Dismiss</button>
            </div>
          </div>
        </div>
      </section>

      <!-- Stale flags section -->
      <section v-if="staleProposals.length > 0" class="proposal-section">
        <h2 class="section-title">
          <span class="section-dot stale-dot"></span>
          Possibly stale
          <span class="section-count">{{ staleProposals.length }}</span>
        </h2>
        <div class="proposal-list">
          <div
            v-for="p in staleProposals"
            :key="p.id"
            class="proposal-card stale-card"
          >
            <div class="card-header" @click="toggleExpanded(p.id)">
              <div class="card-meta">
                <span class="type-badge type-stale">stale</span>
                <span class="card-title">{{ p.title }}</span>
              </div>
              <svg
                class="chevron"
                :class="{ 'is-open': expanded.has(p.id) }"
                viewBox="0 0 16 16"
                fill="currentColor"
              >
                <path fill-rule="evenodd" d="M4.22 6.22a.75.75 0 011.06 0L8 8.94l2.72-2.72a.75.75 0 111.06 1.06l-3.25 3.25a.75.75 0 01-1.06 0L4.22 7.28a.75.75 0 010-1.06z" clip-rule="evenodd" />
              </svg>
            </div>

            <div class="card-description">{{ p.description }}</div>

            <div v-if="expanded.has(p.id)" class="card-expanded">
              <div class="card-reasoning">
                <span class="reasoning-label">Evidence of staleness:</span>
                {{ p.reasoning }}
              </div>
            </div>

            <div class="card-actions">
              <button class="action-btn delete-btn" @click="apply(p)">Delete memory</button>
              <button class="action-btn dismiss-btn" @click="dismiss(p)">Keep it</button>
            </div>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>

<style scoped>
.dream-view {
  max-width: 48rem;
  margin: 0 auto;
  padding: 1.5rem 1.25rem 4rem;
}

/* ── Header ── */
.dream-header {
  margin-bottom: 2rem;
}

.dream-title-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 1rem;
  margin-bottom: 0.5rem;
}

.dream-title {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 1.125rem;
  font-weight: 600;
  color: var(--color-text-primary);
}

.dream-icon {
  width: 1.125rem;
  height: 1.125rem;
  color: var(--color-accent);
}

.dream-description {
  font-size: 0.8125rem;
  color: var(--color-text-muted);
  margin: 0 0 1rem;
  line-height: 1.5;
}

.dream-btn {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.375rem 0.875rem;
  background: var(--color-accent);
  color: #fff;
  border: none;
  border-radius: 0.375rem;
  font-size: 0.8125rem;
  font-weight: 500;
  cursor: pointer;
  white-space: nowrap;
  transition: opacity 0.15s;
}
.dream-btn:hover:not(:disabled) {
  opacity: 0.85;
}
.dream-btn:disabled {
  opacity: 0.6;
  cursor: default;
}
.dream-btn.is-running {
  background: var(--color-accent-muted);
}

.btn-spinner {
  display: inline-block;
  width: 0.75rem;
  height: 0.75rem;
  border: 1.5px solid rgba(255, 255, 255, 0.4);
  border-top-color: #fff;
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
  flex-shrink: 0;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.progress-bar-wrap {
  height: 2px;
  background: var(--color-border);
  border-radius: 1px;
  overflow: hidden;
  margin-bottom: 0.75rem;
}
.progress-bar {
  height: 100%;
  background: var(--color-accent);
  border-radius: 1px;
  transition: width 0.3s ease;
}

.run-summary {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 0.75rem;
  color: var(--color-text-muted);
  flex-wrap: wrap;
}
.summary-num {
  font-weight: 600;
  color: var(--color-text-primary);
}
.summary-dot {
  color: var(--color-border);
}
.summary-err {
  color: #e57373;
}

.dream-error {
  margin-top: 0.75rem;
  padding: 0.625rem 0.875rem;
  background: color-mix(in srgb, #e57373 10%, transparent);
  border: 1px solid color-mix(in srgb, #e57373 30%, transparent);
  border-radius: 0.375rem;
  font-size: 0.8125rem;
  color: #e57373;
}

/* ── Empty state ── */
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.5rem;
  padding: 4rem 1rem;
  text-align: center;
}
.empty-icon {
  width: 2.5rem;
  height: 2.5rem;
  color: var(--color-text-muted);
  opacity: 0.4;
}
.empty-icon svg {
  width: 100%;
  height: 100%;
}
.empty-title {
  font-size: 0.9375rem;
  font-weight: 600;
  color: var(--color-text-primary);
  margin: 0;
}
.empty-sub {
  font-size: 0.8125rem;
  color: var(--color-text-muted);
  margin: 0;
  max-width: 26rem;
  line-height: 1.5;
}

/* ── Bulk row ── */
.bulk-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 1rem;
  margin-bottom: 1.25rem;
}
.bulk-count {
  font-size: 0.8125rem;
  color: var(--color-text-muted);
}
.bulk-actions {
  display: flex;
  gap: 0.5rem;
}
.bulk-btn {
  padding: 0.25rem 0.625rem;
  font-size: 0.75rem;
  border-radius: 0.25rem;
  border: 1px solid var(--color-border);
  background: var(--color-surface);
  color: var(--color-text-muted);
  cursor: pointer;
  transition: all 0.15s;
}
.bulk-btn:hover {
  color: var(--color-text-primary);
  border-color: var(--color-text-muted);
}
.bulk-btn.apply-all:hover {
  color: var(--color-accent);
  border-color: var(--color-accent-muted);
}
.bulk-btn.dismiss-all:hover {
  color: #e57373;
  border-color: color-mix(in srgb, #e57373 40%, transparent);
}

/* ── Sections ── */
.proposal-section {
  margin-bottom: 2rem;
}
.section-title {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 0.75rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: var(--color-text-muted);
  margin: 0 0 0.75rem;
}
.section-dot {
  width: 0.5rem;
  height: 0.5rem;
  border-radius: 50%;
  flex-shrink: 0;
}
.new-dot { background: var(--color-accent); }
.stale-dot { background: #e0a050; }
.section-count {
  margin-left: auto;
  font-weight: 400;
}

/* ── Proposal cards ── */
.proposal-list {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.proposal-card {
  border: 1px solid var(--color-border);
  border-radius: 0.5rem;
  background: var(--color-surface);
  overflow: hidden;
}
.stale-card {
  border-color: color-mix(in srgb, #e0a050 25%, var(--color-border));
}

.card-header {
  display: flex;
  align-items: flex-start;
  gap: 0.75rem;
  padding: 0.75rem 0.875rem 0.5rem;
  cursor: pointer;
  user-select: none;
}
.card-header:hover {
  background: var(--color-surface-hover, rgba(255,255,255,0.03));
}

.card-meta {
  display: flex;
  align-items: baseline;
  gap: 0.5rem;
  flex: 1;
  flex-wrap: wrap;
}

.type-badge {
  font-size: 0.6875rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  padding: 0.1em 0.4em;
  border-radius: 0.25rem;
  flex-shrink: 0;
}
.type-feedback { background: color-mix(in srgb, #7c9ef8 15%, transparent); color: #7c9ef8; }
.type-project  { background: color-mix(in srgb, #63b3a4 15%, transparent); color: #63b3a4; }
.type-user     { background: color-mix(in srgb, #b57bfc 15%, transparent); color: #b57bfc; }
.type-reference{ background: color-mix(in srgb, #f0c060 15%, transparent); color: #f0c060; }
.type-stale    { background: color-mix(in srgb, #e0a050 15%, transparent); color: #e0a050; }

.card-title {
  font-size: 0.875rem;
  font-weight: 500;
  color: var(--color-text-primary);
  line-height: 1.4;
}

.chevron {
  width: 1rem;
  height: 1rem;
  color: var(--color-text-muted);
  flex-shrink: 0;
  transition: transform 0.15s;
  margin-top: 0.1rem;
}
.chevron.is-open {
  transform: rotate(180deg);
}

.card-description {
  padding: 0 0.875rem 0.625rem;
  font-size: 0.8125rem;
  color: var(--color-text-muted);
  line-height: 1.5;
}

.card-expanded {
  padding: 0 0.875rem 0.625rem;
  border-top: 1px solid var(--color-border);
  margin-top: 0.25rem;
}

.card-content {
  font-size: 0.8125rem;
  color: var(--color-text-primary);
  line-height: 1.6;
  white-space: pre-wrap;
  padding: 0.625rem 0;
}

.card-reasoning {
  font-size: 0.75rem;
  color: var(--color-text-muted);
  line-height: 1.5;
  padding: 0.5rem 0;
  border-top: 1px solid var(--color-border);
  margin-top: 0.25rem;
}
.reasoning-label {
  font-weight: 600;
  margin-right: 0.25rem;
}

.card-actions {
  display: flex;
  gap: 0.5rem;
  padding: 0.5rem 0.875rem 0.75rem;
}

.action-btn {
  padding: 0.3125rem 0.75rem;
  font-size: 0.8125rem;
  border-radius: 0.375rem;
  border: 1px solid var(--color-border);
  background: transparent;
  cursor: pointer;
  transition: all 0.15s;
  font-weight: 500;
}
.apply-btn {
  color: var(--color-accent);
  border-color: color-mix(in srgb, var(--color-accent) 40%, transparent);
}
.apply-btn:hover {
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
}
.delete-btn {
  color: #e57373;
  border-color: color-mix(in srgb, #e57373 40%, transparent);
}
.delete-btn:hover {
  background: color-mix(in srgb, #e57373 12%, transparent);
}
.dismiss-btn {
  color: var(--color-text-muted);
}
.dismiss-btn:hover {
  color: var(--color-text-primary);
  border-color: var(--color-text-muted);
}
</style>
