<script setup lang="ts">
import { ref, onMounted, watch } from "vue";
import { useRouter } from "vue-router";
import { useAppStore } from "@/stores/app";
import { useTauri } from "@/composables/useTauri";
import { getVersion } from "@tauri-apps/api/app";
import { save, open } from "@tauri-apps/plugin-dialog";
import type {
  UninstallReport,
  ExportSummary,
  ImportReport,
  ImportMode,
} from "@/types";

const router = useRouter();
const app = useAppStore();
const tauri = useTauri();

const working = ref(false);
const appVersion = ref("...");

const uninstallArmed = ref(false);
const uninstalling = ref(false);
const uninstallReport = ref<UninstallReport | null>(null);
const uninstallError = ref<string | null>(null);
let armTimer: ReturnType<typeof setTimeout> | null = null;

function armUninstall() {
  uninstallArmed.value = true;
  if (armTimer) clearTimeout(armTimer);
  armTimer = setTimeout(() => {
    uninstallArmed.value = false;
  }, 5000);
}

async function confirmUninstall() {
  uninstallArmed.value = false;
  uninstalling.value = true;
  uninstallError.value = null;
  try {
    uninstallReport.value = await tauri.uninstallEverything();
  } catch (e) {
    uninstallError.value = String(e);
  } finally {
    uninstalling.value = false;
  }
}

function cancelUninstall() {
  uninstallArmed.value = false;
  if (armTimer) {
    clearTimeout(armTimer);
    armTimer = null;
  }
}

const exporting = ref(false);
const importing = ref(false);
const lastExport = ref<ExportSummary | null>(null);
const lastImport = ref<ImportReport | null>(null);
const portableError = ref<string | null>(null);
const importMode = ref<ImportMode>("merge");

function defaultExportName(): string {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `claude-memories-${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}.json`;
}

async function exportMemories() {
  portableError.value = null;
  let path: string | null;
  try {
    path = await save({
      title: "Back up memories",
      defaultPath: defaultExportName(),
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
  } catch (e) {
    portableError.value = `Dialog failed: ${e}`;
    return;
  }
  if (!path) return;

  exporting.value = true;
  try {
    lastExport.value = await tauri.exportMemories(path);
  } catch (e) {
    portableError.value = String(e);
  } finally {
    exporting.value = false;
  }
}

async function importMemories() {
  portableError.value = null;
  let picked: string | string[] | null;
  try {
    picked = await open({
      title: "Import memories from JSON",
      multiple: false,
      directory: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
  } catch (e) {
    portableError.value = `Dialog failed: ${e}`;
    return;
  }
  if (!picked) return;
  const path = Array.isArray(picked) ? picked[0] : picked;

  if (importMode.value === "replace") {
    const confirmed = confirm(
      "Replace mode deletes ALL current memories before importing. This cannot be undone. Continue?",
    );
    if (!confirmed) return;
  }

  importing.value = true;
  try {
    lastImport.value = await tauri.importMemories(path, importMode.value);
    await app.loadStatus();
  } catch (e) {
    portableError.value = String(e);
  } finally {
    importing.value = false;
  }
}

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

const splitThresholdInput = ref<number>(app.splitThreshold);
const splitThresholdSaving = ref(false);
const splitThresholdError = ref<string | null>(null);

watch(
  () => app.splitThreshold,
  (v) => {
    splitThresholdInput.value = v;
  },
);

async function saveSplitThreshold() {
  const value = Math.floor(Number(splitThresholdInput.value));
  if (!Number.isFinite(value) || value < 5) {
    splitThresholdError.value = "Threshold must be at least 5.";
    return;
  }
  splitThresholdError.value = null;
  splitThresholdSaving.value = true;
  try {
    await app.setSplitThresholdValue(value);
  } catch (e) {
    splitThresholdError.value = String(e);
  } finally {
    splitThresholdSaving.value = false;
  }
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
    <!-- Header -->
    <div class="page-head">
      <button class="back-link" @click="goHome">
        <span class="back-arrow" aria-hidden="true">←</span>
        Home
      </button>
      <h1 class="page-title">Settings</h1>
    </div>

    <!-- General -->
    <section class="section">
      <h2 class="section-title">General</h2>

      <div class="card">
        <div class="label">Launch at login</div>
        <p class="sub">
          To start automatically on login, open
          <kbd class="kbd">System Settings</kbd>
          <span class="chev">›</span>
          <kbd class="kbd">General</kbd>
          <span class="chev">›</span>
          <kbd class="kbd">Login Items</kbd>
          and add Claude Memory Manager.
        </p>
      </div>
    </section>

    <!-- Claude Code integration -->
    <section class="section">
      <h2 class="section-title">Claude Code integration</h2>

      <!-- MCP Server -->
      <div class="card">
        <div class="row">
          <div class="row-main">
            <div class="label">MCP server</div>
            <div class="sub">
              <span v-if="app.mcpStatus?.registered" class="status-line ok">
                <span class="status-dot"></span>
                Registered in all configs
              </span>
              <span v-else class="status-line warn">
                <span class="status-dot"></span>
                Not registered in all configs
              </span>
            </div>
          </div>
          <div class="button-group">
            <button
              class="primary-btn sm"
              :disabled="working"
              @click="registerMcp"
            >
              {{ working ? "Working…" : "Register" }}
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

        <div v-if="app.mcpStatus?.per_config?.length" class="ledger">
          <div
            v-for="config in app.mcpStatus.per_config"
            :key="config.path"
            class="ledger-row"
          >
            <span class="ledger-dot" :class="{ 'is-ok': config.registered }"></span>
            <span class="ledger-label">{{ config.label }}</span>
            <code class="ledger-path">{{ config.path }}</code>
            <span class="ledger-status" :class="config.registered ? 'is-ok' : 'is-warn'">
              {{ config.registered ? "registered" : "missing" }}
            </span>
          </div>
        </div>
      </div>

      <!-- Hook -->
      <div class="card">
        <div class="row">
          <div class="row-main">
            <div class="label">Auto memory injection</div>
            <p class="sub">
              <span v-if="app.hookStatus?.enabled">
                Claude receives relevant memories on every message. No tool call required.
              </span>
              <span v-else>
                Claude only sees memories when it explicitly calls
                <code class="inline-code">memory_search</code>.
              </span>
            </p>
          </div>
          <button
            class="toggle"
            :class="{ 'is-on': app.hookStatus?.enabled }"
            :aria-pressed="!!app.hookStatus?.enabled"
            aria-label="Toggle auto memory injection"
            @click="toggleHook"
          >
            <span class="toggle-knob"></span>
          </button>
        </div>
        <div v-if="app.hookStatus?.per_config?.length" class="ledger">
          <div
            v-for="config in app.hookStatus.per_config"
            :key="config.path"
            class="ledger-row"
          >
            <span class="ledger-dot" :class="{ 'is-ok': config.installed }"></span>
            <span class="ledger-label">{{ config.label }}</span>
            <code class="ledger-path">{{ config.path }}/settings.json</code>
            <span class="ledger-status" :class="config.installed ? 'is-ok' : 'is-warn'">
              {{ config.installed ? "installed" : "missing" }}
            </span>
          </div>
        </div>
      </div>

      <!-- CLAUDE.md bootstrap -->
      <div class="card">
        <div class="row">
          <div class="row-main">
            <div class="label">CLAUDE.md bootstrap</div>
            <div class="sub">
              <span
                v-if="app.bootstrap?.config_dirs?.every((d) => d.managed_section_present)"
                class="status-line ok"
              >
                <span class="status-dot"></span>
                Installed in all configs
              </span>
              <span
                v-else-if="app.bootstrap?.managed_section_present"
                class="status-line warn"
              >
                <span class="status-dot"></span>
                Partially installed
              </span>
              <span v-else class="status-line warn">
                <span class="status-dot"></span>
                Not installed
              </span>
            </div>
          </div>
        </div>
        <div v-if="app.bootstrap?.config_dirs?.length" class="ledger">
          <div
            v-for="config in app.bootstrap.config_dirs"
            :key="config.path"
            class="ledger-row"
          >
            <span class="ledger-dot" :class="{ 'is-ok': config.managed_section_present }"></span>
            <span class="ledger-label">{{ config.label }}</span>
            <code class="ledger-path">{{ config.path }}/CLAUDE.md</code>
            <span class="ledger-status" :class="config.managed_section_present ? 'is-ok' : 'is-warn'">
              {{ config.managed_section_present ? "managed" : "missing" }}
            </span>
          </div>
        </div>
      </div>
    </section>

    <!-- Memory store -->
    <section class="section">
      <h2 class="section-title">Memory store</h2>

      <div class="card">
        <div class="row">
          <div class="row-main">
            <div class="label">
              <span class="count">{{ app.totalMemories }}</span>
              <span class="count-label">
                memor<template v-if="app.totalMemories === 1">y</template><template v-else>ies</template>
                stored
              </span>
            </div>
            <div class="sub">
              Imported from <code class="inline-code">~/.claude*/projects/**/memory</code>.
            </div>
          </div>
          <button
            class="ghost-btn sm"
            :disabled="working"
            @click="reIngest"
          >
            {{ working ? "Re-ingesting…" : "Re-ingest" }}
          </button>
        </div>
      </div>

      <div v-if="app.lastSetupReport" class="card muted">
        <div class="label">Last ingestion</div>
        <div class="report">
          <span>
            <span class="num">{{ app.lastSetupReport.ingestion.files_scanned }}</span>
            scanned
          </span>
          <span class="sep">·</span>
          <span>
            <span class="num">{{ app.lastSetupReport.ingestion.memories_imported }}</span>
            imported
          </span>
          <span class="sep">·</span>
          <span>
            <span class="num">{{ app.lastSetupReport.ingestion.memories_skipped }}</span>
            deduped
          </span>
        </div>
        <div v-if="app.lastSetupReport.ingestion.errors.length" class="errors">
          <details>
            <summary>
              {{ app.lastSetupReport.ingestion.errors.length }} error{{ app.lastSetupReport.ingestion.errors.length === 1 ? "" : "s" }}
            </summary>
            <ul>
              <li v-for="(err, i) in app.lastSetupReport.ingestion.errors" :key="i">{{ err }}</li>
            </ul>
          </details>
        </div>
      </div>

      <!-- Export -->
      <div class="card">
        <div class="row">
          <div class="row-main">
            <div class="label">Back up memories</div>
            <p class="sub">
              Save a JSON bundle of topics, memories, and edges. Useful before
              a reinstall or when moving machines.
            </p>
          </div>
          <button
            class="ghost-btn sm"
            :disabled="exporting"
            @click="exportMemories"
          >
            {{ exporting ? "Exporting…" : "Export JSON…" }}
          </button>
        </div>
        <div v-if="lastExport" class="trace">
          <span class="trace-check">✓</span>
          Exported
          <span class="num">{{ lastExport.memory_count }}</span>
          memor{{ lastExport.memory_count === 1 ? "y" : "ies" }}
          ({{ Math.round(lastExport.bytes_written / 1024) }} KB) to
          <code class="inline-code">{{ lastExport.path }}</code>
        </div>
      </div>

      <!-- Import -->
      <div class="card">
        <div class="row">
          <div class="row-main">
            <div class="label">Import memories</div>
            <p class="sub">
              Restore a JSON bundle. <strong>Merge</strong> skips duplicates;
              <strong>Replace</strong> wipes everything first.
            </p>
          </div>
          <div class="button-group">
            <div class="seg" role="radiogroup" aria-label="Import mode">
              <button
                type="button"
                class="seg-option"
                :class="{ 'is-active': importMode === 'merge' }"
                :disabled="importing"
                role="radio"
                :aria-checked="importMode === 'merge'"
                @click="importMode = 'merge'"
              >
                Merge
              </button>
              <button
                type="button"
                class="seg-option"
                :class="{ 'is-active': importMode === 'replace' }"
                :disabled="importing"
                role="radio"
                :aria-checked="importMode === 'replace'"
                @click="importMode = 'replace'"
              >
                Replace
              </button>
            </div>
            <button
              class="ghost-btn sm"
              :disabled="importing"
              @click="importMemories"
            >
              {{ importing ? "Importing…" : "Import JSON…" }}
            </button>
          </div>
        </div>
        <div v-if="lastImport" class="trace">
          <span class="trace-check">✓</span>
          Added <span class="num">{{ lastImport.memories_added }}</span>,
          skipped <span class="num">{{ lastImport.memories_skipped }}</span> dup<template v-if="lastImport.memories_skipped !== 1">s</template>,
          topics <span class="num">+{{ lastImport.topics_added }}</span>,
          edges <span class="num">+{{ lastImport.edges_added }}</span><span v-if="lastImport.edges_skipped"> ({{ lastImport.edges_skipped }} skipped)</span>
        </div>
        <div v-if="lastImport?.errors?.length" class="errors">
          <details>
            <summary>
              {{ lastImport.errors.length }} error{{ lastImport.errors.length === 1 ? "" : "s" }}
            </summary>
            <ul>
              <li v-for="(err, i) in lastImport.errors" :key="i">{{ err }}</li>
            </ul>
          </details>
        </div>
      </div>

      <div v-if="portableError" class="error inline-error">
        {{ portableError }}
      </div>
    </section>

    <!-- Organization -->
    <section class="section">
      <h2 class="section-title">Organization</h2>

      <div class="card">
        <div class="row">
          <div class="row-main">
            <div class="label">Auto-organize</div>
            <p class="sub">
              Classify memories into topics and merge duplicates automatically
              when the app opens.
            </p>
          </div>
          <button
            class="toggle"
            :class="{ 'is-on': app.autoOrganize }"
            :aria-pressed="app.autoOrganize"
            aria-label="Toggle auto-organize"
            @click="toggleAutoOrganize"
          >
            <span class="toggle-knob"></span>
          </button>
        </div>
      </div>

      <div class="card">
        <div class="row">
          <div class="row-main">
            <div class="label">Split threshold</div>
            <p class="sub">
              When a topic grows to this many memories, the organizer may
              propose splitting it into narrower sub-topics. Minimum 5. Default
              is 15.
            </p>
            <div v-if="splitThresholdError" class="error inline-error">
              {{ splitThresholdError }}
            </div>
          </div>
          <div class="button-group">
            <input
              v-model.number="splitThresholdInput"
              type="number"
              min="5"
              class="threshold-input"
              :disabled="splitThresholdSaving"
              @keyup.enter="saveSplitThreshold"
            />
            <button
              class="primary-btn sm"
              :disabled="
                splitThresholdSaving ||
                splitThresholdInput === app.splitThreshold
              "
              @click="saveSplitThreshold"
            >
              {{ splitThresholdSaving ? "Saving…" : "Save" }}
            </button>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="row">
          <div class="row-main">
            <div class="label">Organize now</div>
            <div class="sub">
              <span v-if="app.organizing" class="status-line live">
                <span class="status-dot pulse"></span>
                Running
              </span>
              <span v-else-if="app.lastOrganizeReport">
                Last run classified
                <span class="num">{{ app.lastOrganizeReport.classified_count }}</span>,
                merged <span class="num">{{ app.lastOrganizeReport.merged_count }}</span><template v-if="app.lastOrganizeReport.new_topics_created.length">,
                {{ app.lastOrganizeReport.new_topics_created.length }} new topic<template v-if="app.lastOrganizeReport.new_topics_created.length !== 1">s</template></template><template v-if="app.lastOrganizeReport.split_topics?.length">,
                split <span class="num">{{ app.lastOrganizeReport.split_topics.length }}</span> topic<template v-if="app.lastOrganizeReport.split_topics.length !== 1">s</template></template>.
              </span>
              <span v-else>Run a classification and dedup pass right now.</span>
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
              {{ app.organizing ? "Running…" : "Organize" }}
            </button>
          </div>
        </div>
      </div>
    </section>

    <!-- Danger zone -->
    <section class="section">
      <h2 class="section-title danger-section-title">Danger zone</h2>

      <div class="card danger-card" :class="{ 'is-armed': uninstallArmed }">
        <div v-if="!uninstallReport" class="row">
          <div class="row-main">
            <div class="label">Uninstall cleanly</div>
            <p class="sub">
              macOS doesn't run code when you drag the app to Trash, so
              bootstrap artifacts (CLAUDE.md section, settings.json hook, MCP
              registration, <code class="inline-code">~/.claude-memory-manager/</code>)
              would be left behind. Click this before moving the app to Trash.
              <strong class="danger-strong">All memories will be deleted.</strong>
            </p>
          </div>
          <div class="button-group">
            <template v-if="!uninstallArmed">
              <button
                class="danger-btn"
                :disabled="uninstalling"
                @click="armUninstall"
              >
                Uninstall…
              </button>
            </template>
            <template v-else>
              <button
                class="ghost-btn sm"
                :disabled="uninstalling"
                @click="cancelUninstall"
              >
                Cancel
              </button>
              <button
                class="danger-confirm-btn"
                :disabled="uninstalling"
                @click="confirmUninstall"
              >
                {{ uninstalling ? "Uninstalling…" : "Yes, delete everything" }}
              </button>
            </template>
          </div>
        </div>

        <div v-else class="uninstall-result">
          <div class="label">
            <span
              v-if="uninstallReport.data_dir_removed && uninstallReport.steps.every((s) => s.success)"
              class="status-line ok"
            >
              <span class="status-dot"></span>
              Uninstalled cleanly
            </span>
            <span v-else class="status-line warn">
              <span class="status-dot"></span>
              Completed with warnings
            </span>
          </div>
          <p class="sub">
            You can now quit this app and drag
            <code class="inline-code">Claude Memory Manager.app</code> to the Trash.
            {{ uninstallReport.data_dir_removed
              ? "Data directory removed."
              : `Data directory still at ${uninstallReport.data_dir_path}.` }}
          </p>
          <ul class="uninstall-steps">
            <li
              v-for="(step, i) in uninstallReport.steps"
              :key="i"
              :class="{ ok: step.success, warn: !step.success }"
            >
              <span class="step-glyph">{{ step.success ? "✓" : "✗" }}</span>
              <span>{{ step.label }}</span>
              <span v-if="step.error" class="step-error">— {{ step.error }}</span>
            </li>
          </ul>
        </div>

        <div v-if="uninstallError" class="error inline-error">
          {{ uninstallError }}
        </div>
      </div>
    </section>

    <!-- Footer -->
    <footer class="footer">
      Claude Memory Manager · v{{ appVersion }}
    </footer>

    <div v-if="app.error" class="error">{{ app.error }}</div>
  </div>
</template>

<style scoped>
.settings {
  max-width: 42rem;
  margin: 0 auto;
  padding: 1.5rem 1.5rem 3rem;
}

/* Header */
.page-head {
  margin-bottom: 2rem;
}
.back-link {
  display: inline-flex;
  align-items: center;
  gap: 0.3125rem;
  padding: 0.25rem 0.5rem 0.25rem 0.375rem;
  margin-left: -0.5rem;
  background: none;
  border: none;
  color: var(--color-text-muted);
  font-size: 0.75rem;
  cursor: pointer;
  border-radius: 0.25rem;
  font-family: inherit;
  transition: color 0.15s ease, background 0.15s ease;
}
.back-link:hover {
  color: var(--color-text-primary);
  background: var(--color-surface-hover);
}
.back-arrow {
  display: inline-block;
  transition: transform 0.2s ease;
}
.back-link:hover .back-arrow {
  transform: translateX(-2px);
}
.page-title {
  margin: 0.75rem 0 0;
  font-size: 1.5rem;
  font-weight: 600;
  color: var(--color-text-primary);
  letter-spacing: -0.02em;
}

/* Section */
.section {
  margin-bottom: 2rem;
}
.section-title {
  margin: 0 0 0.625rem;
  font-size: 0.6875rem;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--color-text-muted);
}
.danger-section-title {
  color: var(--color-health-error);
}

/* Card */
.card {
  border: 1px solid var(--color-border);
  background: var(--color-surface-alt);
  border-radius: 0.5rem;
  padding: 0.875rem 1rem;
  margin-bottom: 0.375rem;
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
.row-main {
  min-width: 0;
  flex: 1;
}

/* Typography */
.label {
  font-size: 0.8125rem;
  font-weight: 500;
  color: var(--color-text-primary);
}
.sub {
  margin: 0.1875rem 0 0;
  font-size: 0.75rem;
  line-height: 1.5;
  color: var(--color-text-secondary);
}

.inline-code {
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  font-size: 0.6875rem;
  padding: 0.0625rem 0.25rem;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 0.1875rem;
  color: var(--color-accent);
}
.kbd {
  display: inline-block;
  padding: 0.0625rem 0.3125rem;
  border: 1px solid var(--color-border-light);
  border-radius: 0.1875rem;
  background: var(--color-surface);
  font-family: inherit;
  font-size: 0.6875rem;
  color: var(--color-text-primary);
  line-height: 1.4;
}
.chev {
  color: var(--color-text-muted);
  padding: 0 0.125rem;
}

/* Memory-count inline lockup */
.count {
  font-size: 1.125rem;
  font-weight: 600;
  color: var(--color-accent);
  font-variant-numeric: tabular-nums;
  margin-right: 0.375rem;
}
.count-label {
  font-weight: 500;
  color: var(--color-text-primary);
}

/* Inline status line */
.status-line {
  display: inline-flex;
  align-items: center;
  gap: 0.4375rem;
  font-size: 0.75rem;
}
.status-line.ok { color: var(--color-health-ok); }
.status-line.warn { color: var(--color-health-warning); }
.status-line.live { color: var(--color-accent); }
.status-dot {
  width: 0.375rem;
  height: 0.375rem;
  border-radius: 50%;
  background: currentColor;
  flex-shrink: 0;
  opacity: 0.9;
}
.status-dot.pulse {
  animation: pulse 1.4s ease-in-out infinite;
}
@keyframes pulse {
  0%, 100% { opacity: 0.5; }
  50% { opacity: 1; }
}

/* Ledger (per-config) */
.ledger {
  margin-top: 0.75rem;
  padding-top: 0.625rem;
  border-top: 1px solid var(--color-border);
  display: flex;
  flex-direction: column;
}
.ledger-row {
  display: grid;
  grid-template-columns: 0.75rem minmax(4rem, auto) 1fr auto;
  align-items: center;
  gap: 0.625rem;
  padding: 0.3125rem 0;
  font-size: 0.6875rem;
}
.ledger-row + .ledger-row {
  border-top: 1px solid color-mix(in srgb, var(--color-border) 50%, transparent);
}
.ledger-dot {
  width: 0.375rem;
  height: 0.375rem;
  border-radius: 50%;
  background: var(--color-health-warning);
  justify-self: center;
}
.ledger-dot.is-ok {
  background: var(--color-health-ok);
}
.ledger-label {
  color: var(--color-text-primary);
  font-weight: 500;
  text-transform: capitalize;
}
.ledger-path {
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  color: var(--color-text-muted);
  font-size: 0.625rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ledger-status {
  text-transform: uppercase;
  font-size: 0.5625rem;
  letter-spacing: 0.08em;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}
.ledger-status.is-ok { color: var(--color-health-ok); }
.ledger-status.is-warn { color: var(--color-health-warning); }

/* Report line */
.report {
  margin-top: 0.375rem;
  font-size: 0.75rem;
  color: var(--color-text-secondary);
  display: flex;
  gap: 0.5rem;
  flex-wrap: wrap;
  align-items: baseline;
}
.num {
  color: var(--color-text-primary);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}
.sep {
  color: var(--color-border-light);
}

/* Trace (post-action result line) */
.trace {
  margin-top: 0.625rem;
  padding-top: 0.625rem;
  border-top: 1px solid var(--color-border);
  font-size: 0.6875rem;
  color: var(--color-text-secondary);
  line-height: 1.55;
  word-break: break-word;
}
.trace-check {
  color: var(--color-health-ok);
  margin-right: 0.3125rem;
  font-weight: 600;
}

/* Buttons */
.button-group {
  display: flex;
  gap: 0.5rem;
  flex-shrink: 0;
  align-items: center;
}

.primary-btn {
  display: inline-flex;
  align-items: center;
  padding: 0.375rem 0.875rem;
  font-size: 0.6875rem;
  font-weight: 500;
  background: var(--color-accent);
  color: #1a1410;
  border: 1px solid transparent;
  border-radius: 0.3125rem;
  cursor: pointer;
  font-family: inherit;
  transition: background 0.15s ease;
}
.primary-btn:hover:not(:disabled) {
  background: var(--color-accent-hover);
}
.primary-btn:disabled { opacity: 0.5; cursor: not-allowed; }

.threshold-input {
  width: 4.5rem;
  padding: 0.375rem 0.5rem;
  font-size: 0.75rem;
  font-family: inherit;
  background: var(--color-bg);
  color: var(--color-text);
  border: 1px solid var(--color-border);
  border-radius: 0.3125rem;
  text-align: right;
}
.threshold-input:focus {
  outline: none;
  border-color: var(--color-accent);
}
.threshold-input:disabled { opacity: 0.5; cursor: not-allowed; }

.ghost-btn {
  padding: 0.375rem 0.875rem;
  font-size: 0.6875rem;
  background: transparent;
  color: var(--color-text-secondary);
  border: 1px solid var(--color-border);
  border-radius: 0.3125rem;
  cursor: pointer;
  font-family: inherit;
  transition: border-color 0.15s ease, color 0.15s ease;
}
.ghost-btn:hover:not(:disabled) {
  color: var(--color-text-primary);
  border-color: var(--color-border-light);
}
.ghost-btn:disabled { opacity: 0.5; cursor: not-allowed; }

/* Segmented Merge/Replace */
.seg {
  display: inline-flex;
  padding: 0.1875rem;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 0.3125rem;
}
.seg-option {
  padding: 0.1875rem 0.5625rem;
  font-size: 0.6875rem;
  font-family: inherit;
  color: var(--color-text-muted);
  background: transparent;
  border: none;
  border-radius: 0.1875rem;
  cursor: pointer;
  transition: background 0.15s ease, color 0.15s ease;
}
.seg-option:hover:not(:disabled):not(.is-active) {
  color: var(--color-text-secondary);
}
.seg-option.is-active {
  background: var(--color-surface-active);
  color: var(--color-text-primary);
  font-weight: 600;
}
.seg-option:disabled { opacity: 0.5; cursor: not-allowed; }

/* Toggle */
.toggle {
  position: relative;
  width: 2.25rem;
  height: 1.25rem;
  border-radius: 999px;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  cursor: pointer;
  flex-shrink: 0;
  padding: 0;
  transition: background 0.15s ease, border-color 0.15s ease;
}
.toggle.is-on {
  background: var(--color-accent);
  border-color: var(--color-accent);
}
.toggle-knob {
  position: absolute;
  top: 1px;
  left: 1px;
  width: 1rem;
  height: 1rem;
  background: var(--color-text-secondary);
  border-radius: 50%;
  transition: transform 0.18s ease, background 0.15s ease;
}
.toggle.is-on .toggle-knob {
  transform: translateX(1rem);
  background: #1a1410;
}

/* Errors */
.errors {
  margin-top: 0.625rem;
  font-size: 0.6875rem;
  color: var(--color-health-error);
}
.errors summary { cursor: pointer; }
.errors ul { margin: 0.375rem 0 0; padding-left: 1rem; }

.error {
  margin-top: 0.75rem;
  padding: 0.625rem 0.875rem;
  border: 1px solid color-mix(in srgb, var(--color-health-error) 30%, transparent);
  background: color-mix(in srgb, var(--color-health-error) 8%, transparent);
  color: var(--color-health-error);
  border-radius: 0.3125rem;
  font-size: 0.75rem;
}
.inline-error { margin-top: 0.625rem; }

/* Danger zone */
.danger-card {
  border-color: color-mix(in srgb, var(--color-health-error) 30%, var(--color-border));
  transition: border-color 0.15s ease, box-shadow 0.15s ease;
}
.danger-card.is-armed {
  border-color: var(--color-health-error);
  box-shadow: 0 0 0 1px color-mix(in srgb, var(--color-health-error) 25%, transparent);
}
.danger-strong {
  color: var(--color-health-error);
  font-weight: 600;
}

.danger-btn {
  padding: 0.375rem 0.875rem;
  font-size: 0.6875rem;
  font-family: inherit;
  color: var(--color-health-error);
  background: transparent;
  border: 1px solid color-mix(in srgb, var(--color-health-error) 40%, var(--color-border));
  border-radius: 0.3125rem;
  cursor: pointer;
  transition: border-color 0.15s ease, background 0.15s ease;
}
.danger-btn:hover:not(:disabled) {
  border-color: var(--color-health-error);
  background: color-mix(in srgb, var(--color-health-error) 6%, transparent);
}
.danger-btn:disabled { opacity: 0.5; cursor: not-allowed; }

.danger-confirm-btn {
  padding: 0.375rem 0.875rem;
  font-size: 0.6875rem;
  font-weight: 600;
  font-family: inherit;
  color: #1a0a0a;
  background: var(--color-health-error);
  border: 1px solid var(--color-health-error);
  border-radius: 0.3125rem;
  cursor: pointer;
  transition: background 0.15s ease;
}
.danger-confirm-btn:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-health-error) 88%, black);
}
.danger-confirm-btn:disabled { opacity: 0.6; cursor: not-allowed; }

/* Uninstall result */
.uninstall-result {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}
.uninstall-steps {
  margin: 0.25rem 0 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  font-size: 0.6875rem;
}
.uninstall-steps li {
  display: flex;
  align-items: baseline;
  gap: 0.5rem;
}
.uninstall-steps li.ok { color: var(--color-health-ok); }
.uninstall-steps li.warn { color: var(--color-health-warning); }
.step-glyph {
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  flex-shrink: 0;
}
.step-error {
  color: var(--color-text-muted);
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  font-size: 0.625rem;
}

/* Footer */
.footer {
  margin-top: 2.5rem;
  text-align: center;
  font-size: 0.6875rem;
  color: var(--color-text-muted);
}
</style>
