<script setup lang="ts">
import { ref, onMounted } from "vue";
import { useRouter } from "vue-router";
import { useAppStore } from "@/stores/app";
import { getVersion } from "@tauri-apps/api/app";

const router = useRouter();
const app = useAppStore();

const working = ref(false);
const appVersion = ref("...");

getVersion().then((v) => (appVersion.value = v));

onMounted(() => {
  app.loadStatus();
});

async function registerMcp() {
  working.value = true;
  try {
    await app.registerMcp();
  } finally {
    working.value = false;
  }
}

async function unregisterMcp() {
  working.value = true;
  try {
    await app.unregisterMcp();
  } finally {
    working.value = false;
  }
}

async function reIngest() {
  working.value = true;
  try {
    await app.runSetup();
  } finally {
    working.value = false;
  }
}

async function toggleAutoOrganize() {
  await app.setAutoOrganizeEnabled(!app.autoOrganize);
}

async function toggleHook() {
  if (app.hookStatus?.enabled) {
    await app.disableHook();
  } else {
    await app.enableHook();
  }
}

async function organizeNow() {
  try {
    await app.runOrganize();
  } catch {
    /* error is in store */
  }
}

async function undoLast() {
  try {
    await app.undoLast();
  } catch {
    /* error is in store */
  }
}

function goHome() {
  router.push({ name: "home" });
}
</script>

<template>
  <div class="settings">
    <button class="back-btn" @click="goHome">
      <svg viewBox="0 0 16 16" fill="currentColor" class="back-icon">
        <path d="M9.78 12.78a.75.75 0 01-1.06 0L4.47 8.53a.75.75 0 010-1.06l4.25-4.25a.75.75 0 111.06 1.06L6.06 8l3.72 3.72a.75.75 0 010 1.06z" />
      </svg>
      Home
    </button>

    <h1 class="title">Settings</h1>

    <!-- Claude Code connection -->
    <section class="section">
      <h2 class="section-title">Claude Code Integration</h2>
      <div class="card">
        <div class="row">
          <div>
            <div class="label">MCP Server</div>
            <div class="sub">
              <span v-if="app.mcpStatus?.registered" class="ok">✓ Registered in all configs</span>
              <span v-else class="warn">Not registered in all configs</span>
            </div>
          </div>
          <div class="button-group">
            <button
              class="primary-btn sm"
              :disabled="working"
              @click="registerMcp"
            >
              {{ working ? "Working..." : "Register / Re-register" }}
            </button>
            <button
              v-if="app.mcpStatus?.registered"
              class="ghost-btn sm"
              :disabled="working"
              @click="unregisterMcp"
            >
              Unregister
            </button>
          </div>
        </div>

        <!-- Per-config status table -->
        <div v-if="app.mcpStatus?.per_config?.length" class="per-config">
          <div
            v-for="config in app.mcpStatus.per_config"
            :key="config.path"
            class="config-row"
          >
            <span class="config-dot" :class="{ 'is-ok': config.registered }"></span>
            <span class="config-label">{{ config.label }}</span>
            <code class="config-path">{{ config.path }}</code>
            <span v-if="config.registered" class="config-status ok">registered</span>
            <span v-else class="config-status warn">not registered</span>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="row">
          <div>
            <div class="label">Auto memory injection (UserPromptSubmit hook)</div>
            <div class="sub">
              <span v-if="app.hookStatus?.enabled" class="ok">
                ✓ Enabled — Claude receives relevant memories on every message
              </span>
              <span v-else class="warn">Disabled — Claude only sees memories when it calls memory_search</span>
            </div>
          </div>
          <button
            class="toggle"
            :class="{ 'is-on': app.hookStatus?.enabled }"
            @click="toggleHook"
          >
            <span class="toggle-knob"></span>
          </button>
        </div>
        <div v-if="app.hookStatus?.per_config?.length" class="per-config">
          <div
            v-for="config in app.hookStatus.per_config"
            :key="config.path"
            class="config-row"
          >
            <span class="config-dot" :class="{ 'is-ok': config.installed }"></span>
            <span class="config-label">{{ config.label }}</span>
            <code class="config-path">{{ config.path }}/settings.json</code>
            <span v-if="config.installed" class="config-status ok">installed</span>
            <span v-else class="config-status warn">not installed</span>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="row">
          <div>
            <div class="label">CLAUDE.md Bootstrap</div>
            <div class="sub">
              <span v-if="app.bootstrap?.config_dirs?.every((d) => d.managed_section_present)" class="ok">
                ✓ Installed in all configs
              </span>
              <span v-else-if="app.bootstrap?.managed_section_present" class="warn">
                Partially installed
              </span>
              <span v-else class="warn">Not installed</span>
            </div>
          </div>
        </div>
        <div v-if="app.bootstrap?.config_dirs?.length" class="per-config">
          <div
            v-for="config in app.bootstrap.config_dirs"
            :key="config.path"
            class="config-row"
          >
            <span class="config-dot" :class="{ 'is-ok': config.managed_section_present }"></span>
            <span class="config-label">{{ config.label }}</span>
            <code class="config-path">{{ config.path }}/CLAUDE.md</code>
            <span v-if="config.managed_section_present" class="config-status ok">managed</span>
            <span v-else class="config-status warn">missing</span>
          </div>
        </div>
      </div>
    </section>

    <!-- Memory store -->
    <section class="section">
      <h2 class="section-title">Memory Store</h2>
      <div class="card">
        <div class="row">
          <div>
            <div class="label">Total memories</div>
            <div class="sub">{{ app.totalMemories }} stored</div>
          </div>
          <button
            class="ghost-btn sm"
            :disabled="working"
            @click="reIngest"
          >
            {{ working ? "Re-ingesting..." : "Re-ingest existing files" }}
          </button>
        </div>
      </div>

      <div v-if="app.lastSetupReport" class="card">
        <div class="label">Last setup</div>
        <div class="sub report">
          Scanned {{ app.lastSetupReport.ingestion.files_scanned }} files ·
          Imported {{ app.lastSetupReport.ingestion.memories_imported }} ·
          Skipped {{ app.lastSetupReport.ingestion.memories_skipped }} (dedup)
        </div>
        <div v-if="app.lastSetupReport.ingestion.errors.length" class="errors">
          <details>
            <summary>{{ app.lastSetupReport.ingestion.errors.length }} error{{ app.lastSetupReport.ingestion.errors.length === 1 ? "" : "s" }}</summary>
            <ul>
              <li v-for="(err, i) in app.lastSetupReport.ingestion.errors" :key="i">{{ err }}</li>
            </ul>
          </details>
        </div>
      </div>
    </section>

    <!-- Organization -->
    <section class="section">
      <h2 class="section-title">Organization</h2>
      <div class="card">
        <div class="row">
          <div>
            <div class="label">Auto-organize</div>
            <div class="sub">
              Automatically classify memories into topics and merge duplicates when the app opens.
            </div>
          </div>
          <button
            class="toggle"
            :class="{ 'is-on': app.autoOrganize }"
            @click="toggleAutoOrganize"
          >
            <span class="toggle-knob"></span>
          </button>
        </div>
      </div>

      <div class="card">
        <div class="row">
          <div>
            <div class="label">Organize now</div>
            <div class="sub">
              <span v-if="app.organizing">Running...</span>
              <span v-else-if="app.lastOrganizeReport">
                Last run: classified {{ app.lastOrganizeReport.classified_count }},
                merged {{ app.lastOrganizeReport.merged_count }},
                new topics: {{ app.lastOrganizeReport.new_topics_created.length }}
              </span>
              <span v-else>Run a classification + dedup pass right now.</span>
            </div>
          </div>
          <div class="button-group">
            <button
              class="ghost-btn sm"
              :disabled="app.organizing"
              @click="undoLast"
            >
              Undo last
            </button>
            <button
              class="primary-btn sm"
              :disabled="app.organizing"
              @click="organizeNow"
            >
              {{ app.organizing ? "Running..." : "Organize" }}
            </button>
          </div>
        </div>
      </div>
    </section>

    <!-- About -->
    <section class="section">
      <h2 class="section-title">About</h2>
      <div class="card muted">
        <div class="label">Claude Memory Manager</div>
        <div class="sub">v{{ appVersion }} · Memory autopilot for Claude Code</div>
      </div>
    </section>

    <div v-if="app.error" class="error">{{ app.error }}</div>
  </div>
</template>

<style scoped>
.settings {
  max-width: 42rem;
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

.title {
  font-size: 1.5rem;
  font-weight: 600;
  color: var(--color-text-primary);
  margin: 0 0 1.5rem;
  letter-spacing: -0.02em;
}

.section {
  margin-bottom: 2rem;
}
.section-title {
  font-size: 0.6875rem;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--color-text-muted);
  margin: 0 0 0.75rem;
}

.card {
  border: 1px solid var(--color-border);
  background: var(--color-surface-alt);
  border-radius: 0.5rem;
  padding: 1rem 1.25rem;
  margin-bottom: 0.5rem;
}
.card.muted {
  background: transparent;
  border-style: dashed;
}

.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 1rem;
}

.label {
  font-size: 0.8125rem;
  color: var(--color-text-primary);
  font-weight: 500;
}
.sub {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
  margin-top: 0.125rem;
}
.sub.report {
  color: var(--color-text-secondary);
}

.ok {
  color: var(--color-health-ok);
}
.warn {
  color: var(--color-health-warning);
}

.path {
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  font-size: 0.625rem;
  color: var(--color-text-muted);
  margin-top: 0.5rem;
  word-break: break-all;
}

.per-config {
  margin-top: 0.875rem;
  padding-top: 0.75rem;
  border-top: 1px solid var(--color-border);
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
}
.config-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  font-size: 0.6875rem;
}
.config-dot {
  width: 0.375rem;
  height: 0.375rem;
  border-radius: 50%;
  background: var(--color-health-warning);
  flex-shrink: 0;
}
.config-dot.is-ok {
  background: var(--color-health-ok);
}
.config-label {
  color: var(--color-text-primary);
  font-weight: 500;
  text-transform: capitalize;
  min-width: 4rem;
}
.config-path {
  flex: 1;
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  color: var(--color-text-muted);
  font-size: 0.625rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.config-status {
  flex-shrink: 0;
  text-transform: uppercase;
  font-size: 0.5625rem;
  letter-spacing: 0.05em;
  font-weight: 600;
}
.config-status.ok {
  color: var(--color-health-ok);
}
.config-status.warn {
  color: var(--color-health-warning);
}

.primary-btn {
  display: inline-flex;
  align-items: center;
  padding: 0.5rem 1rem;
  font-size: 0.75rem;
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
  font-size: 0.6875rem;
}

.ghost-btn {
  padding: 0.5rem 1rem;
  font-size: 0.75rem;
  background: none;
  color: var(--color-text-secondary);
  border: 1px solid var(--color-border);
  border-radius: 0.375rem;
  cursor: pointer;
  transition: border-color 0.15s, color 0.15s;
}
.ghost-btn:hover:not(:disabled) {
  color: var(--color-text-primary);
  border-color: var(--color-border-light);
}
.ghost-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.ghost-btn.sm {
  padding: 0.375rem 0.875rem;
  font-size: 0.6875rem;
}

.errors {
  margin-top: 0.75rem;
  font-size: 0.6875rem;
  color: var(--color-health-error);
}
.errors summary {
  cursor: pointer;
}
.errors ul {
  margin: 0.5rem 0 0;
  padding-left: 1rem;
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

.button-group {
  display: flex;
  gap: 0.5rem;
}

.toggle {
  position: relative;
  width: 2.5rem;
  height: 1.375rem;
  border-radius: 999px;
  background: var(--color-border);
  border: none;
  cursor: pointer;
  transition: background 0.15s;
  flex-shrink: 0;
}
.toggle.is-on {
  background: var(--color-accent);
}
.toggle-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 1rem;
  height: 1rem;
  background: var(--color-text-primary);
  border-radius: 50%;
  transition: transform 0.2s ease;
}
.toggle.is-on .toggle-knob {
  transform: translateX(1.125rem);
  background: var(--color-surface);
}
</style>
