<script setup lang="ts">
import { computed, onMounted } from "vue";
import { useRouter } from "vue-router";
import { useAppStore } from "@/stores/app";
import {
  ERR_NO_CLAUDE_INSTALL,
  ERR_NO_CLAUDE_CLI,
  type Topic,
} from "@/types";

const router = useRouter();
const app = useAppStore();

onMounted(() => {
  if (!app.needsSetup) {
    app.loadTopics();
  }
});

const claudeCodeMissing = computed(
  () => app.bootstrap && !app.bootstrap.claude_code_installed,
);
const claudeCliMissing = computed(
  () =>
    app.bootstrap &&
    app.bootstrap.claude_code_installed &&
    !app.bootstrap.claude_cli_available,
);
const startupErrors = computed(() => app.bootstrap?.startup_errors ?? []);

const errorDisplay = computed(() => {
  const err = app.error ?? "";
  if (err.includes(ERR_NO_CLAUDE_INSTALL)) {
    return "Claude Code is not installed. Install it from https://claude.com/claude-code, then reopen this app.";
  }
  if (err.includes(ERR_NO_CLAUDE_CLI)) {
    return "The `claude` CLI was not found on your PATH. Install Claude Code, then reopen this app.";
  }
  return err;
});

async function refreshStatus() {
  await app.loadStatus();
}

const sortedTopics = computed(() =>
  [...app.topics].sort((a, b) => b.memory_count - a.memory_count),
);

const untopicCount = computed(() => {
  const total = app.totalMemories;
  const assigned = app.topics.reduce((sum, t) => sum + t.memory_count, 0);
  return Math.max(total - assigned, 0);
});

function goToTopic(topic: Topic) {
  router.push({ name: "topic", params: { name: topic.name } });
}

function goToUntopiced() {
  router.push({ name: "topic", params: { name: "__untopiced__" } });
}

async function setupNow() {
  try {
    await app.runSetup();
  } catch {
    /* error is in store */
  }
}

async function connectClaude() {
  try {
    await app.registerMcp();
  } catch {
    /* error is in store */
  }
}

async function organizeNow() {
  try {
    await app.runOrganize();
  } catch {
    /* error is in store */
  }
}
</script>

<template>
  <div class="home">
    <!-- Status banner -->
    <div v-if="app.bootstrap" class="status">
      <div class="status-row">
        <span class="status-dot" :class="{
          'is-ok': !app.needsSetup && !app.needsMcpRegistration,
          'is-warn': app.needsSetup || app.needsMcpRegistration,
        }"></span>
        <span class="status-text" v-if="!app.needsSetup && !app.needsMcpRegistration">
          Connected to Claude Code · {{ app.totalMemories }} memor{{ app.totalMemories === 1 ? "y" : "ies" }}
        </span>
        <span class="status-text" v-else-if="app.needsSetup">
          Not set up yet
        </span>
        <span class="status-text" v-else-if="app.needsMcpRegistration">
          Memory store ready · MCP not registered
        </span>
      </div>
    </div>

    <!-- Organizer progress -->
    <div v-if="app.organizing" class="organize-banner">
      <span class="organize-spinner" aria-hidden="true"></span>
      <div class="organize-body">
        <div class="organize-title">
          {{ app.organizeProgress?.message ?? "Organizing memories…" }}
        </div>
        <div
          v-if="app.organizeProgress && app.organizeProgress.total > 0"
          class="organize-meta"
        >
          {{ app.organizeProgress.phase }} · step
          {{ app.organizeProgress.current + 1 }} of
          {{ app.organizeProgress.total }}
        </div>
        <div
          v-else-if="app.organizeProgress"
          class="organize-meta"
        >
          {{ app.organizeProgress.phase }}
        </div>
      </div>
    </div>

    <!-- Startup errors (directory creation, DB init) -->
    <div v-if="startupErrors.length > 0" class="blocking-card warn">
      <h2 class="blocking-title">Startup issues</h2>
      <ul class="blocking-list">
        <li v-for="(msg, i) in startupErrors" :key="i">{{ msg }}</li>
      </ul>
      <p class="blocking-hint">
        See <code>~/.claude-memory-manager/startup.log</code> for details.
      </p>
    </div>

    <!-- Blocking state: Claude Code not installed -->
    <div v-if="claudeCodeMissing" class="blocking-card">
      <h2 class="blocking-title">Install Claude Code first</h2>
      <p class="blocking-lead">
        No <code>~/.claude*</code> directory was found on this machine. This
        app augments Claude Code with a persistent memory system, so Claude
        Code needs to be installed before we can do anything.
      </p>
      <p class="blocking-lead">
        Install it from
        <a href="https://claude.com/claude-code" target="_blank" rel="noopener">claude.com/claude-code</a>,
        run it once so it creates <code>~/.claude</code>, then reopen this app.
      </p>
      <button class="primary-btn sm" @click="refreshStatus">Re-check</button>
    </div>

    <!-- Blocking state: claude CLI not on PATH -->
    <div v-else-if="claudeCliMissing" class="blocking-card">
      <h2 class="blocking-title">Claude CLI not on PATH</h2>
      <p class="blocking-lead">
        We found your Claude Code config directory, but running
        <code>claude --version</code> failed. We need the <code>claude</code>
        binary to be on <code>PATH</code> so we can register the memory MCP
        server.
      </p>
      <p class="blocking-lead">
        Make sure Claude Code is installed and that launching a shell gives
        you a working <code>claude</code> command. If you installed via an app
        bundle, you may need to reopen this Memory Manager app from a
        terminal that has Claude on <code>PATH</code>.
      </p>
      <button class="primary-btn sm" @click="refreshStatus">Re-check</button>
    </div>

    <!-- First-time setup -->
    <div v-else-if="app.needsSetup" class="setup-card">
      <div class="setup-icon">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <path stroke-linecap="round" stroke-linejoin="round" d="M9.813 15.904L9 18.75l-.813-2.846a4.5 4.5 0 00-3.09-3.09L2.25 12l2.846-.813a4.5 4.5 0 003.09-3.09L9 5.25l.813 2.846a4.5 4.5 0 003.09 3.09L15.75 12l-2.846.813a4.5 4.5 0 00-3.09 3.09z" />
        </svg>
      </div>
      <h1 class="setup-title">Welcome to Memory Autopilot</h1>
      <p class="setup-lead">
        A self-organizing memory system for Claude Code. Set it up once and Claude
        automatically has context in every session — no matter which directory
        you start from.
      </p>
      <div class="setup-steps">
        <div class="step">
          <span class="step-num">1</span>
          <div class="step-body">
            <div class="step-title">Import your existing memories</div>
            <div class="step-desc">
              Scans <code>~/.claude/projects/*/memory/</code> and imports everything into a central store.
            </div>
          </div>
        </div>
        <div class="step">
          <span class="step-num">2</span>
          <div class="step-body">
            <div class="step-title">Bootstrap CLAUDE.md</div>
            <div class="step-desc">
              Adds a managed section to <code>~/.claude/CLAUDE.md</code> that teaches Claude to use the memory tools.
            </div>
          </div>
        </div>
        <div class="step">
          <span class="step-num">3</span>
          <div class="step-body">
            <div class="step-title">Register MCP server</div>
            <div class="step-desc">
              Installs a user-scope MCP server so <code>memory_search</code>, <code>memory_add</code>, and friends are available in every Claude Code session.
            </div>
          </div>
        </div>
        <div class="step">
          <span class="step-num">4</span>
          <div class="step-body">
            <div class="step-title">Auto-organize</div>
            <div class="step-desc">
              Imported memories are classified into topics and duplicates merged. Runs automatically on each launch.
            </div>
          </div>
        </div>
      </div>
      <button class="primary-btn large" :disabled="app.settingUp" @click="setupNow">
        {{ app.settingUp ? "Setting up..." : "Get started" }}
      </button>
      <div v-if="app.lastSetupReport" class="setup-result">
        <p>
          ✓ Imported {{ app.lastSetupReport.ingestion.memories_imported }} memories from
          {{ app.lastSetupReport.ingestion.files_scanned }} files.
        </p>
        <p v-if="app.lastSetupReport.mcp_registrations.length > 0">
          MCP registered in
          {{ app.lastSetupReport.mcp_registrations.filter((r) => r.success).length }}
          of {{ app.lastSetupReport.mcp_registrations.length }} config{{
            app.lastSetupReport.mcp_registrations.length === 1 ? "" : "s"
          }}:
        </p>
        <ul v-if="app.lastSetupReport.mcp_registrations.length > 0" class="registration-list">
          <li
            v-for="reg in app.lastSetupReport.mcp_registrations"
            :key="reg.path"
          >
            <span v-if="reg.success" class="ok-text">✓</span>
            <span v-else class="warn-text">✗</span>
            {{ reg.label }} ({{ reg.path }})
            <span v-if="reg.error" class="warn-text error-detail">{{ reg.error }}</span>
          </li>
        </ul>
      </div>
    </div>

    <!-- MCP registration prompt (only if setup is done but MCP isn't) -->
    <div
      v-else-if="app.needsMcpRegistration && !claudeCodeMissing && !claudeCliMissing"
      class="action-card"
    >
      <div class="action-head">
        <div>
          <div class="action-title">Connect to Claude Code</div>
          <div class="action-desc">Register the memory MCP server so Claude Code can search and write memories in every session.</div>
        </div>
        <button class="primary-btn sm" @click="connectClaude">Connect</button>
      </div>
    </div>

    <!-- Topics grid -->
    <div
      v-if="!app.needsSetup && !claudeCodeMissing && !claudeCliMissing"
      class="topics-section"
    >
      <div class="section-head">
        <h2 class="section-title">Topics</h2>
        <div class="section-actions">
          <span v-if="app.organizing" class="organizing-state">
            <span class="spinner"></span>
            Organizing...
          </span>
          <button
            v-else-if="app.lastOrganizeReport && app.lastOrganizeReport.classified_count + app.lastOrganizeReport.merged_count > 0"
            class="ghost-btn xs"
            @click="app.lastOrganizeReport = null"
            title="Dismiss"
          >
            ✓ Organized {{ app.lastOrganizeReport.classified_count }} ·
            Merged {{ app.lastOrganizeReport.merged_count }}
          </button>
          <button
            v-else
            class="ghost-btn xs"
            :disabled="app.organizing"
            @click="organizeNow"
          >
            Organize now
          </button>
          <span class="section-meta">{{ app.topics.length }} topic{{ app.topics.length === 1 ? "" : "s" }}</span>
        </div>
      </div>

      <div v-if="app.loading && app.topics.length === 0" class="empty">Loading...</div>

      <div v-else class="topic-grid">
        <button
          v-for="topic in sortedTopics"
          :key="topic.name"
          class="topic-card"
          @click="goToTopic(topic)"
        >
          <div class="topic-count">{{ topic.memory_count }}</div>
          <div class="topic-name">{{ topic.name }}</div>
          <div v-if="topic.description" class="topic-desc">{{ topic.description }}</div>
        </button>

        <button
          v-if="untopicCount > 0"
          class="topic-card is-untopiced"
          @click="goToUntopiced"
        >
          <div class="topic-count">{{ untopicCount }}</div>
          <div class="topic-name">Unclassified</div>
          <div class="topic-desc">Memories waiting for a topic</div>
        </button>
      </div>

      <div v-if="!app.loading && app.topics.length === 0 && untopicCount === 0" class="empty">
        No memories yet. Claude will add them as you work.
      </div>
    </div>

    <div v-if="errorDisplay" class="error">{{ errorDisplay }}</div>
  </div>
</template>

<style scoped>
.home {
  max-width: 56rem;
  margin: 0 auto;
  padding: 2rem 1.5rem;
}

.status {
  margin-bottom: 1.5rem;
}
.status-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 0.75rem;
  color: var(--color-text-muted);
}
.status-dot {
  width: 0.5rem;
  height: 0.5rem;
  border-radius: 50%;
  background: var(--color-text-muted);
}
.status-dot.is-ok {
  background: var(--color-health-ok);
  box-shadow: 0 0 8px color-mix(in srgb, var(--color-health-ok) 50%, transparent);
}
.status-dot.is-warn {
  background: var(--color-health-warning);
}
.status-text {
  letter-spacing: 0.01em;
}

.organize-banner {
  display: flex;
  align-items: center;
  gap: 0.875rem;
  padding: 0.875rem 1rem;
  margin-bottom: 1.25rem;
  border: 1px solid color-mix(in srgb, var(--color-accent) 35%, transparent);
  background: color-mix(in srgb, var(--color-accent) 10%, var(--color-surface-alt));
  border-radius: 0.625rem;
}
.organize-spinner {
  width: 0.875rem;
  height: 0.875rem;
  border-radius: 50%;
  border: 2px solid color-mix(in srgb, var(--color-accent) 30%, transparent);
  border-top-color: var(--color-accent);
  animation: organize-spin 0.9s linear infinite;
  flex-shrink: 0;
}
@keyframes organize-spin {
  to { transform: rotate(360deg); }
}
.organize-body {
  display: flex;
  flex-direction: column;
  gap: 0.125rem;
  min-width: 0;
}
.organize-title {
  font-size: 0.875rem;
  color: var(--color-text);
}
.organize-meta {
  font-size: 0.7rem;
  color: var(--color-text-muted);
  text-transform: lowercase;
}

/* Setup card */
.setup-card {
  border: 1px solid var(--color-border);
  background: var(--color-surface-alt);
  border-radius: 0.75rem;
  padding: 2.5rem 2rem;
  max-width: 40rem;
  margin: 2rem auto;
  text-align: center;
}
.setup-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 3.5rem;
  height: 3.5rem;
  margin: 0 auto 1.25rem;
  border-radius: 50%;
  background: color-mix(in srgb, var(--color-accent) 15%, transparent);
  color: var(--color-accent);
}
.setup-icon svg {
  width: 1.75rem;
  height: 1.75rem;
}
.setup-title {
  font-size: 1.5rem;
  font-weight: 600;
  color: var(--color-text-primary);
  margin: 0 0 0.75rem;
  letter-spacing: -0.02em;
}
.setup-lead {
  font-size: 0.875rem;
  color: var(--color-text-secondary);
  line-height: 1.6;
  margin: 0 auto 2rem;
  max-width: 32rem;
}
.setup-lead code,
.setup-body code,
.step-desc code {
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  font-size: 0.6875rem;
  background: var(--color-surface);
  padding: 0.0625rem 0.375rem;
  border-radius: 0.25rem;
  color: var(--color-accent);
}
.setup-steps {
  display: flex;
  flex-direction: column;
  gap: 0.875rem;
  margin-bottom: 2rem;
  text-align: left;
}
.step {
  display: flex;
  gap: 0.875rem;
  align-items: flex-start;
}
.step-num {
  flex-shrink: 0;
  width: 1.5rem;
  height: 1.5rem;
  border-radius: 50%;
  background: color-mix(in srgb, var(--color-accent) 15%, transparent);
  color: var(--color-accent);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 0.75rem;
  font-weight: 600;
}
.step-title {
  font-size: 0.8125rem;
  color: var(--color-text-primary);
  font-weight: 500;
}
.step-desc {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
  margin-top: 0.125rem;
}
.step-desc code {
  font-family: ui-monospace, monospace;
  font-size: 0.625rem;
  color: var(--color-accent);
}
.setup-result {
  font-size: 0.75rem;
  color: var(--color-health-ok);
  margin: 1rem 0 0;
}

/* Blocking card (no-install / no-cli / startup errors) */
.blocking-card {
  border: 1px solid var(--color-border);
  background: var(--color-surface-alt);
  border-radius: 0.75rem;
  padding: 1.75rem 1.75rem;
  max-width: 40rem;
  margin: 2rem auto;
}
.blocking-card.warn {
  border-color: var(--color-health-warning);
}
.blocking-title {
  font-size: 1.125rem;
  font-weight: 600;
  color: var(--color-text-primary);
  margin: 0 0 0.75rem;
  letter-spacing: -0.01em;
}
.blocking-lead {
  font-size: 0.875rem;
  color: var(--color-text-secondary);
  line-height: 1.55;
  margin: 0 0 0.875rem;
}
.blocking-list {
  margin: 0 0 0.875rem;
  padding-left: 1.25rem;
  font-size: 0.8125rem;
  color: var(--color-text-secondary);
  line-height: 1.55;
}
.blocking-hint {
  font-size: 0.75rem;
  color: var(--color-text-muted);
  margin: 0 0 0.875rem;
}
.blocking-card code,
.blocking-card a {
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  font-size: 0.8125rem;
  color: var(--color-accent);
}
.blocking-card a {
  font-family: inherit;
  text-decoration: underline;
}

/* Action card */
.action-card {
  border: 1px solid var(--color-border);
  background: var(--color-surface-alt);
  border-radius: 0.5rem;
  padding: 1rem 1.25rem;
  margin-bottom: 1.5rem;
}
.action-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 1rem;
}
.action-title {
  font-size: 0.875rem;
  font-weight: 500;
  color: var(--color-text-primary);
}
.action-desc {
  font-size: 0.75rem;
  color: var(--color-text-muted);
  margin-top: 0.125rem;
}

/* Primary button */
.primary-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0.625rem 1.25rem;
  font-size: 0.8125rem;
  font-weight: 500;
  background: var(--color-accent);
  color: var(--color-surface);
  border: none;
  border-radius: 0.375rem;
  cursor: pointer;
  transition: background 0.15s;
}
.primary-btn:hover:not(:disabled) {
  background: var(--color-accent-hover);
}
.primary-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.primary-btn.sm {
  padding: 0.375rem 0.875rem;
  font-size: 0.75rem;
}
.primary-btn.large {
  padding: 0.75rem 2rem;
  font-size: 0.875rem;
  font-weight: 600;
}

.warn-text {
  color: var(--color-health-warning);
}
.ok-text {
  color: var(--color-health-ok);
}
.setup-result {
  margin-top: 1.5rem;
  text-align: left;
  font-size: 0.75rem;
  color: var(--color-text-secondary);
}
.setup-result p {
  margin: 0 0 0.5rem;
}
.registration-list {
  list-style: none;
  padding: 0;
  margin: 0.5rem 0 0;
  font-family: ui-monospace, Menlo, monospace;
  font-size: 0.6875rem;
}
.registration-list li {
  padding: 0.25rem 0;
  color: var(--color-text-muted);
}
.error-detail {
  display: block;
  margin-left: 1rem;
  margin-top: 0.125rem;
}

/* Topics */
.topics-section {
  margin-top: 1rem;
}
.section-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  margin-bottom: 1rem;
}
.section-title {
  font-size: 0.6875rem;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--color-text-muted);
  margin: 0;
}
.section-meta {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
}
.section-actions {
  display: flex;
  align-items: center;
  gap: 0.75rem;
}
.organizing-state {
  display: inline-flex;
  align-items: center;
  gap: 0.375rem;
  font-size: 0.6875rem;
  color: var(--color-accent);
}
.spinner {
  display: inline-block;
  width: 0.625rem;
  height: 0.625rem;
  border: 1.5px solid color-mix(in srgb, var(--color-accent) 30%, transparent);
  border-top-color: var(--color-accent);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}
@keyframes spin {
  to { transform: rotate(360deg); }
}
.ghost-btn {
  background: none;
  border: 1px solid var(--color-border);
  color: var(--color-text-secondary);
  padding: 0.25rem 0.5rem;
  border-radius: 0.25rem;
  cursor: pointer;
  font-size: 0.6875rem;
  transition: border-color 0.15s, color 0.15s;
}
.ghost-btn:hover:not(:disabled) {
  border-color: var(--color-accent-muted);
  color: var(--color-accent);
}
.ghost-btn.xs {
  padding: 0.1875rem 0.5rem;
  font-size: 0.625rem;
}
.ghost-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.topic-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(10rem, 1fr));
  gap: 0.75rem;
}
.topic-card {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  padding: 1rem;
  border: 1px solid var(--color-border);
  background: var(--color-surface-alt);
  border-radius: 0.5rem;
  text-align: left;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s, transform 0.1s;
}
.topic-card:hover {
  background: var(--color-surface-hover);
  border-color: var(--color-border-light);
  transform: translateY(-1px);
}
.topic-card.is-untopiced {
  border-style: dashed;
  background: transparent;
}
.topic-card.is-untopiced:hover {
  background: var(--color-surface-hover);
}
.topic-count {
  font-size: 1.375rem;
  font-weight: 600;
  color: var(--color-accent);
  font-variant-numeric: tabular-nums;
  line-height: 1.2;
}
.topic-name {
  font-size: 0.8125rem;
  color: var(--color-text-primary);
  font-weight: 500;
  text-transform: capitalize;
}
.topic-desc {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
  line-height: 1.4;
}

.empty {
  text-align: center;
  padding: 3rem 1rem;
  color: var(--color-text-muted);
  font-size: 0.8125rem;
}

.error {
  margin-top: 1rem;
  padding: 0.75rem 1rem;
  border: 1px solid color-mix(in srgb, var(--color-health-error) 30%, transparent);
  background: color-mix(in srgb, var(--color-health-error) 10%, transparent);
  color: var(--color-health-error);
  border-radius: 0.375rem;
  font-size: 0.75rem;
}
</style>
