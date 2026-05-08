<script setup lang="ts">
import { onMounted, ref, computed } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { useTauri } from "@/composables/useTauri";
import { useAppStore } from "@/stores/app";
import type { RepoGraph, RepoEdge, ScanProposal } from "@/types";

const tauri = useTauri();
const app = useAppStore();

const graph = ref<RepoGraph | null>(null);
const loading = ref(true);
const activeNamespace = ref<string | null>(null);
const selectedNode = ref<string | null>(null);
const selectedEdge = ref<RepoEdge | null>(null);

// Manual add form
const showAddForm = ref(false);
const addForm = ref({ source_repo: "", target_repo: "", relationship_type: "calls", evidence: "", namespace: "default" });
const addSaving = ref(false);
const addError = ref("");

// Scan wizard
const showScanWizard = ref(false);
const scanNamespace = ref("default");
const scanProposals = ref<ScanProposal[]>([]);
const scanChecked = ref<Set<number>>(new Set());
const scanLoading = ref(false);
const scanSaving = ref(false);
const scanError = ref("");
const scanDone = ref(false);

const RELATIONSHIP_TYPES = ["calls", "imports", "shares-schema", "deploys-to", "extends"];

// SVG dimensions
const SVG_W = 800;
const SVG_H = 520;
const CX = SVG_W / 2;
const CY = SVG_H / 2;
const RADIUS = 190;
const NODE_R = 28;

interface NodePos {
  id: string;
  x: number;
  y: number;
  label: string;
  edgeCount: number;
}

const filteredEdges = computed(() => {
  if (!graph.value) return [];
  if (!activeNamespace.value) return graph.value.edges;
  return graph.value.edges.filter((e) => e.namespace === activeNamespace.value);
});

const filteredNodes = computed(() => {
  const nodeSet = new Set<string>();
  for (const e of filteredEdges.value) {
    nodeSet.add(e.source_repo);
    nodeSet.add(e.target_repo);
  }
  return [...nodeSet].sort();
});

const nodes = computed<NodePos[]>(() => {
  const ns = filteredNodes.value;
  if (ns.length === 0) return [];
  if (ns.length === 1) {
    return [{ id: ns[0], x: CX, y: CY, label: shortName(ns[0]), edgeCount: 0 }];
  }
  return ns.map((id, i) => {
    const angle = (2 * Math.PI * i) / ns.length - Math.PI / 2;
    const r = ns.length <= 4 ? RADIUS * 0.7 : RADIUS;
    return {
      id,
      x: CX + r * Math.cos(angle),
      y: CY + r * Math.sin(angle),
      label: shortName(id),
      edgeCount: edgeCountFor(id),
    };
  });
});

const nodeMap = computed(() => {
  const m: Record<string, NodePos> = {};
  for (const n of nodes.value) m[n.id] = n;
  return m;
});

function edgeCountFor(id: string): number {
  return filteredEdges.value.filter((e) => e.source_repo === id || e.target_repo === id).length;
}

function shortName(path: string): string {
  const parts = path.replace(/\\/g, "/").split("/");
  return parts[parts.length - 1] || path;
}

function edgePath(edge: RepoEdge): string {
  const s = nodeMap.value[edge.source_repo];
  const t = nodeMap.value[edge.target_repo];
  if (!s || !t) return "";
  const dx = t.x - s.x;
  const dy = t.y - s.y;
  const len = Math.sqrt(dx * dx + dy * dy);
  if (len === 0) return "";
  const mx = (s.x + t.x) / 2 - (dy / len) * 20;
  const my = (s.y + t.y) / 2 + (dx / len) * 20;
  return `M ${s.x} ${s.y} Q ${mx} ${my} ${t.x} ${t.y}`;
}

function nodeColor(id: string): string {
  const palette = [
    "#7c6af5", "#4fa8e0", "#56c97b", "#f0a653",
    "#e05c7c", "#50c8b8", "#c87850", "#8880d4",
  ];
  let hash = 0;
  for (let i = 0; i < id.length; i++) hash = (hash * 31 + id.charCodeAt(i)) & 0xffff;
  return palette[hash % palette.length];
}

function isSelectedEdge(edge: RepoEdge): boolean {
  if (!selectedNode.value) return selectedEdge.value?.id === edge.id;
  return edge.source_repo === selectedNode.value || edge.target_repo === selectedNode.value;
}

function selectNode(id: string) {
  selectedEdge.value = null;
  selectedNode.value = selectedNode.value === id ? null : id;
}

function selectEdge(edge: RepoEdge) {
  selectedNode.value = null;
  selectedEdge.value = selectedEdge.value?.id === edge.id ? null : edge;
}

function clearSelection() {
  selectedNode.value = null;
  selectedEdge.value = null;
}

const selectedNodeEdges = computed<RepoEdge[]>(() => {
  if (!selectedNode.value || !graph.value) return [];
  return filteredEdges.value.filter(
    (e) => e.source_repo === selectedNode.value || e.target_repo === selectedNode.value,
  );
});

function setNamespace(ns: string | null) {
  activeNamespace.value = ns;
  clearSelection();
}

async function load() {
  loading.value = true;
  try {
    graph.value = await tauri.getRepoGraph();
    if (activeNamespace.value && !graph.value.namespaces.includes(activeNamespace.value)) {
      activeNamespace.value = null;
    }
  } catch (e) {
    app.error = String(e);
  } finally {
    loading.value = false;
  }
}

// Manual add form
function openAddForm() {
  addForm.value = { source_repo: "", target_repo: "", relationship_type: "calls", evidence: "", namespace: activeNamespace.value ?? "default" };
  addError.value = "";
  showAddForm.value = true;
}

async function submitAdd() {
  addError.value = "";
  const f = addForm.value;
  if (!f.source_repo.trim() || !f.target_repo.trim() || !f.namespace.trim()) {
    addError.value = "Source repo, target repo, and namespace are required.";
    return;
  }
  if (f.source_repo.trim() === f.target_repo.trim()) {
    addError.value = "Source and target cannot be the same.";
    return;
  }
  addSaving.value = true;
  try {
    await tauri.addRepoEdge(f.source_repo.trim(), f.target_repo.trim(), f.relationship_type, f.evidence.trim(), f.namespace.trim());
    showAddForm.value = false;
    await load();
  } catch (e) {
    addError.value = String(e);
  } finally {
    addSaving.value = false;
  }
}

async function deleteEdge(id: number) {
  try {
    await tauri.deleteRepoEdge(id);
    if (selectedEdge.value?.id === id) selectedEdge.value = null;
    await load();
  } catch (e) {
    app.error = String(e);
  }
}

// Scan wizard
function openScanWizard() {
  scanProposals.value = [];
  scanChecked.value = new Set();
  scanError.value = "";
  scanDone.value = false;
  scanNamespace.value = activeNamespace.value ?? "default";
  showScanWizard.value = true;
}

async function pickAndScan() {
  scanError.value = "";
  scanLoading.value = true;
  try {
    const dir = await open({ directory: true, multiple: false, title: "Select directory containing repos" });
    if (!dir) {
      scanLoading.value = false;
      return;
    }
    const proposals = await tauri.scanReposInDirectory(dir as string);
    scanProposals.value = proposals;
    scanChecked.value = new Set(proposals.map((_, i) => i));
    if (proposals.length === 0) {
      scanError.value = "No relationships found. Try a directory that contains multiple git repos with .env.example or composer.json files.";
    }
  } catch (e) {
    scanError.value = String(e);
  } finally {
    scanLoading.value = false;
  }
}

function toggleScanProposal(i: number) {
  const s = new Set(scanChecked.value);
  if (s.has(i)) s.delete(i);
  else s.add(i);
  scanChecked.value = s;
}

function toggleAllProposals() {
  if (scanChecked.value.size === scanProposals.value.length) {
    scanChecked.value = new Set();
  } else {
    scanChecked.value = new Set(scanProposals.value.map((_, i) => i));
  }
}

async function confirmScan() {
  if (scanChecked.value.size === 0) return;
  scanSaving.value = true;
  scanError.value = "";
  try {
    const ns = scanNamespace.value.trim() || "default";
    const selected = [...scanChecked.value].map((i) => scanProposals.value[i]);
    await Promise.all(
      selected.map((p) =>
        tauri.addRepoEdge(p.source_repo, p.target_repo, p.relationship_type, p.evidence, ns),
      ),
    );
    scanDone.value = true;
    await load();
  } catch (e) {
    scanError.value = String(e);
  } finally {
    scanSaving.value = false;
  }
}

onMounted(load);
</script>

<template>
  <div class="graph-view">
    <div class="graph-header">
      <div class="graph-header-top">
        <div>
          <h1 class="graph-title">Repo Relationship Graph</h1>
          <p class="graph-subtitle">
            Track inter-service dependencies across your repos.
            Claude calls <code>repo_link</code> when it discovers one, or add them manually.
          </p>
        </div>
        <div class="header-actions">
          <button class="btn btn-secondary" @click="openScanWizard">Scan Directory</button>
          <button class="btn btn-primary" @click="openAddForm">+ Add Relationship</button>
        </div>
      </div>

      <!-- Namespace tabs -->
      <div v-if="graph && graph.namespaces.length > 0" class="ns-tabs">
        <button
          class="ns-tab"
          :class="{ 'is-active': activeNamespace === null }"
          @click="setNamespace(null)"
        >All</button>
        <button
          v-for="ns in graph.namespaces"
          :key="ns"
          class="ns-tab"
          :class="{ 'is-active': activeNamespace === ns }"
          @click="setNamespace(ns)"
        >{{ ns }}</button>
      </div>
    </div>

    <div v-if="loading" class="empty">Loading graph...</div>

    <div v-else-if="filteredNodes.length === 0" class="empty-state">
      <div class="empty-icon">
        <svg viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="1.5">
          <circle cx="12" cy="24" r="6" />
          <circle cx="36" cy="12" r="6" />
          <circle cx="36" cy="36" r="6" />
          <line x1="18" y1="24" x2="30" y2="14" />
          <line x1="18" y1="24" x2="30" y2="34" />
        </svg>
      </div>
      <p class="empty-title">No relationships recorded yet</p>
      <p class="empty-hint">
        Click <strong>Scan Directory</strong> to auto-detect relationships from a folder of repos,
        or <strong>+ Add Relationship</strong> to add one manually.
      </p>
      <div class="empty-actions">
        <button class="btn btn-secondary" @click="openScanWizard">Scan Directory</button>
        <button class="btn btn-primary" @click="openAddForm">+ Add Relationship</button>
      </div>
    </div>

    <div v-else class="graph-layout">
      <div class="graph-canvas-wrap">
        <svg
          class="graph-svg"
          :viewBox="`0 0 ${SVG_W} ${SVG_H}`"
          @click.self="clearSelection"
        >
          <defs>
            <marker id="arrowhead" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
              <polygon points="0 0, 8 3, 0 6" fill="var(--color-text-muted)" opacity="0.6" />
            </marker>
            <marker id="arrowhead-active" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
              <polygon points="0 0, 8 3, 0 6" fill="var(--color-accent)" />
            </marker>
          </defs>

          <g class="edges">
            <path
              v-for="edge in filteredEdges"
              :key="edge.id"
              :d="edgePath(edge)"
              class="edge-path"
              :class="{ 'is-active': isSelectedEdge(edge) }"
              fill="none"
              :marker-end="isSelectedEdge(edge) ? 'url(#arrowhead-active)' : 'url(#arrowhead)'"
              @click.stop="selectEdge(edge)"
            />
          </g>

          <g
            v-for="node in nodes"
            :key="node.id"
            class="node-group"
            :class="{ 'is-selected': selectedNode === node.id }"
            @click.stop="selectNode(node.id)"
          >
            <circle
              :cx="node.x"
              :cy="node.y"
              :r="NODE_R"
              class="node-circle"
              :fill="nodeColor(node.id)"
              :opacity="selectedNode && selectedNode !== node.id ? 0.35 : 0.9"
            />
            <text :x="node.x" :y="node.y + 4" text-anchor="middle" class="node-label">
              {{ node.label.length > 12 ? node.label.slice(0, 11) + "…" : node.label }}
            </text>
            <text
              v-if="node.edgeCount > 0"
              :x="node.x + NODE_R - 4"
              :y="node.y - NODE_R + 8"
              text-anchor="middle"
              class="node-badge"
            >{{ node.edgeCount }}</text>
          </g>
        </svg>
      </div>

      <!-- Detail panel -->
      <div class="detail-panel">
        <div v-if="selectedNode" class="detail-card">
          <div class="detail-head">
            <div class="detail-node-dot" :style="{ background: nodeColor(selectedNode) }"></div>
            <span class="detail-node-name">{{ shortName(selectedNode) }}</span>
          </div>
          <div class="detail-path">{{ selectedNode }}</div>
          <div v-if="selectedNodeEdges.length === 0" class="detail-empty">No recorded relationships.</div>
          <div v-else class="detail-edges">
            <div v-for="edge in selectedNodeEdges" :key="edge.id" class="detail-edge-item">
              <div class="detail-edge-row">
                <span class="edge-dir">{{ edge.source_repo === selectedNode ? "→" : "←" }}</span>
                <span class="rel-badge" :class="`rel-${edge.relationship_type}`">{{ edge.relationship_type }}</span>
                <span class="detail-edge-target">{{ shortName(edge.source_repo === selectedNode ? edge.target_repo : edge.source_repo) }}</span>
                <button class="del-btn" title="Delete" @click.stop="deleteEdge(edge.id)">×</button>
              </div>
              <div v-if="edge.evidence" class="detail-evidence">{{ edge.evidence }}</div>
              <div class="detail-ns-tag">{{ edge.namespace }}</div>
            </div>
          </div>
        </div>

        <div v-else-if="selectedEdge" class="detail-card">
          <div class="detail-head">
            <span class="detail-node-name">{{ shortName(selectedEdge.source_repo) }}</span>
            <span class="detail-arrow">→</span>
            <span class="detail-node-name">{{ shortName(selectedEdge.target_repo) }}</span>
          </div>
          <div class="detail-head" style="margin-top: 0.375rem; gap: 0.5rem;">
            <span class="rel-badge" :class="`rel-${selectedEdge.relationship_type}`">{{ selectedEdge.relationship_type }}</span>
            <span class="detail-ns-tag">{{ selectedEdge.namespace }}</span>
          </div>
          <div v-if="selectedEdge.evidence" class="detail-evidence" style="margin-top: 0.75rem">{{ selectedEdge.evidence }}</div>
          <button class="del-btn-full" @click="deleteEdge(selectedEdge.id)">Delete relationship</button>
        </div>

        <div v-else class="detail-hint">
          <p>Click a node or edge for details.</p>
          <div class="graph-stats">
            <div class="stat">
              <span class="stat-num">{{ filteredNodes.length }}</span>
              <span class="stat-label">repos</span>
            </div>
            <div class="stat">
              <span class="stat-num">{{ filteredEdges.length }}</span>
              <span class="stat-label">deps</span>
            </div>
            <div class="stat" v-if="graph">
              <span class="stat-num">{{ graph.namespaces.length }}</span>
              <span class="stat-label">namespaces</span>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- Add Relationship Modal -->
    <div v-if="showAddForm" class="modal-backdrop" @click.self="showAddForm = false">
      <div class="modal">
        <div class="modal-header">
          <h2 class="modal-title">Add Relationship</h2>
          <button class="modal-close" @click="showAddForm = false">×</button>
        </div>
        <div class="modal-body">
          <div class="form-row">
            <label class="form-label">Source Repo</label>
            <input v-model="addForm.source_repo" class="form-input" placeholder="e.g. shopify-sanity-connector" />
          </div>
          <div class="form-row">
            <label class="form-label">Target Repo</label>
            <input v-model="addForm.target_repo" class="form-input" placeholder="e.g. loyalty-service" />
          </div>
          <div class="form-row">
            <label class="form-label">Relationship Type</label>
            <select v-model="addForm.relationship_type" class="form-input">
              <option v-for="t in RELATIONSHIP_TYPES" :key="t" :value="t">{{ t }}</option>
            </select>
          </div>
          <div class="form-row">
            <label class="form-label">Evidence <span class="form-optional">(optional)</span></label>
            <input v-model="addForm.evidence" class="form-input" placeholder="e.g. config/services.php — LOYALTY_SERVICE_URL" />
          </div>
          <div class="form-row">
            <label class="form-label">Namespace</label>
            <input v-model="addForm.namespace" class="form-input" placeholder="e.g. hobbii, personal, default" list="ns-datalist" />
            <datalist id="ns-datalist">
              <option v-for="ns in (graph?.namespaces ?? [])" :key="ns" :value="ns" />
            </datalist>
          </div>
          <p v-if="addError" class="form-error">{{ addError }}</p>
        </div>
        <div class="modal-footer">
          <button class="btn btn-secondary" @click="showAddForm = false">Cancel</button>
          <button class="btn btn-primary" :disabled="addSaving" @click="submitAdd">
            {{ addSaving ? "Saving…" : "Add Relationship" }}
          </button>
        </div>
      </div>
    </div>

    <!-- Scan Wizard Modal -->
    <div v-if="showScanWizard" class="modal-backdrop" @click.self="showScanWizard = false">
      <div class="modal modal-wide">
        <div class="modal-header">
          <h2 class="modal-title">Scan Directory for Relationships</h2>
          <button class="modal-close" @click="showScanWizard = false">×</button>
        </div>

        <div v-if="scanDone" class="modal-body scan-done">
          <div class="scan-done-icon">✓</div>
          <p class="scan-done-title">{{ [...scanChecked].length }} relationship{{ [...scanChecked].length === 1 ? '' : 's' }} saved</p>
          <button class="btn btn-primary" @click="showScanWizard = false">Done</button>
        </div>

        <div v-else class="modal-body">
          <p class="scan-intro">
            Pick a directory that contains multiple git repos. The scanner will look for URL env vars
            and composer/package.json imports that reference sibling repos.
          </p>

          <div class="scan-ns-row">
            <label class="form-label">Namespace for imported relationships</label>
            <input v-model="scanNamespace" class="form-input form-input-inline" placeholder="e.g. hobbii" list="ns-datalist-scan" />
            <datalist id="ns-datalist-scan">
              <option v-for="ns in (graph?.namespaces ?? [])" :key="ns" :value="ns" />
            </datalist>
          </div>

          <button class="btn btn-secondary scan-pick-btn" :disabled="scanLoading" @click="pickAndScan">
            {{ scanLoading ? "Scanning…" : "Pick Directory & Scan" }}
          </button>

          <p v-if="scanError" class="form-error">{{ scanError }}</p>

          <div v-if="scanProposals.length > 0" class="scan-proposals">
            <div class="scan-proposals-header">
              <span class="scan-proposals-count">{{ scanProposals.length }} relationships found</span>
              <button class="btn-link" @click="toggleAllProposals">
                {{ scanChecked.size === scanProposals.length ? "Deselect all" : "Select all" }}
              </button>
            </div>
            <div class="proposals-list">
              <label
                v-for="(p, i) in scanProposals"
                :key="i"
                class="proposal-item"
                :class="{ 'is-checked': scanChecked.has(i) }"
              >
                <input type="checkbox" :checked="scanChecked.has(i)" @change="toggleScanProposal(i)" class="proposal-checkbox" />
                <div class="proposal-body">
                  <div class="proposal-rel">
                    <span class="proposal-repo">{{ p.source_repo }}</span>
                    <span class="rel-badge" :class="`rel-${p.relationship_type}`">{{ p.relationship_type }}</span>
                    <span class="proposal-repo">{{ p.target_repo }}</span>
                  </div>
                  <div class="proposal-evidence">{{ p.evidence }}</div>
                </div>
              </label>
            </div>
          </div>
        </div>

        <div v-if="!scanDone" class="modal-footer">
          <button class="btn btn-secondary" @click="showScanWizard = false">Cancel</button>
          <button
            v-if="scanProposals.length > 0"
            class="btn btn-primary"
            :disabled="scanSaving || scanChecked.size === 0"
            @click="confirmScan"
          >
            {{ scanSaving ? "Saving…" : `Save ${scanChecked.size} relationship${scanChecked.size === 1 ? '' : 's'}` }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.graph-view {
  max-width: 64rem;
  margin: 0 auto;
  padding: 1.5rem;
}

.graph-header {
  margin-bottom: 1.5rem;
}
.graph-header-top {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 1rem;
  margin-bottom: 0.875rem;
}
.graph-title {
  font-size: 1.25rem;
  font-weight: 600;
  color: var(--color-text-primary);
  margin: 0 0 0.375rem;
  letter-spacing: -0.02em;
}
.graph-subtitle {
  font-size: 0.8125rem;
  color: var(--color-text-muted);
  margin: 0;
}
.graph-subtitle code {
  font-family: ui-monospace, monospace;
  font-size: 0.75rem;
  color: var(--color-accent);
}
.header-actions {
  display: flex;
  gap: 0.5rem;
  flex-shrink: 0;
}

/* Namespace tabs */
.ns-tabs {
  display: flex;
  gap: 0.25rem;
  flex-wrap: wrap;
}
.ns-tab {
  padding: 0.25rem 0.75rem;
  font-size: 0.75rem;
  font-weight: 500;
  border-radius: 1rem;
  border: 1px solid var(--color-border);
  background: transparent;
  color: var(--color-text-secondary);
  cursor: pointer;
  transition: background 0.15s, color 0.15s, border-color 0.15s;
}
.ns-tab:hover {
  background: var(--color-surface-alt);
  color: var(--color-text-primary);
}
.ns-tab.is-active {
  background: var(--color-accent);
  border-color: var(--color-accent);
  color: white;
}

/* Buttons */
.btn {
  padding: 0.375rem 0.875rem;
  font-size: 0.8125rem;
  font-weight: 500;
  border-radius: 0.375rem;
  border: 1px solid transparent;
  cursor: pointer;
  transition: opacity 0.15s, background 0.15s;
  white-space: nowrap;
}
.btn:disabled { opacity: 0.5; cursor: not-allowed; }
.btn-primary {
  background: var(--color-accent);
  color: white;
  border-color: var(--color-accent);
}
.btn-primary:hover:not(:disabled) { opacity: 0.88; }
.btn-secondary {
  background: var(--color-surface-alt);
  color: var(--color-text-secondary);
  border-color: var(--color-border);
}
.btn-secondary:hover:not(:disabled) {
  background: var(--color-surface);
  color: var(--color-text-primary);
}
.btn-link {
  background: none;
  border: none;
  color: var(--color-accent);
  font-size: 0.75rem;
  cursor: pointer;
  padding: 0;
}
.btn-link:hover { text-decoration: underline; }

/* Empty state */
.empty {
  text-align: center;
  padding: 4rem 1rem;
  color: var(--color-text-muted);
  font-size: 0.8125rem;
}
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 1rem;
  padding: 4rem 2rem;
  text-align: center;
}
.empty-icon { width: 4rem; height: 4rem; color: var(--color-text-muted); opacity: 0.4; }
.empty-title { font-size: 1rem; font-weight: 500; color: var(--color-text-secondary); margin: 0; }
.empty-hint { font-size: 0.8125rem; color: var(--color-text-muted); max-width: 28rem; line-height: 1.6; margin: 0; }
.empty-actions { display: flex; gap: 0.5rem; }

/* Graph layout */
.graph-layout {
  display: grid;
  grid-template-columns: 1fr 18rem;
  gap: 1.25rem;
  align-items: start;
}
.graph-canvas-wrap {
  background: var(--color-surface-alt);
  border: 1px solid var(--color-border);
  border-radius: 0.75rem;
  overflow: hidden;
}
.graph-svg { width: 100%; height: auto; display: block; cursor: default; }

.edge-path {
  stroke: var(--color-text-muted);
  stroke-width: 1.5;
  opacity: 0.35;
  cursor: pointer;
  transition: opacity 0.15s, stroke 0.15s;
}
.edge-path:hover, .edge-path.is-active {
  stroke: var(--color-accent);
  opacity: 1;
  stroke-width: 2;
}
.node-group { cursor: pointer; }
.node-circle { transition: opacity 0.2s, filter 0.2s; }
.node-group:hover .node-circle, .node-group.is-selected .node-circle {
  filter: brightness(1.15) drop-shadow(0 0 6px currentColor);
  opacity: 1 !important;
}
.node-label { font-size: 0.6875rem; font-weight: 600; fill: white; pointer-events: none; user-select: none; }
.node-badge { font-size: 0.5625rem; font-weight: 700; fill: white; pointer-events: none; }

/* Detail panel */
.detail-panel { display: flex; flex-direction: column; gap: 0.75rem; }
.detail-card {
  background: var(--color-surface-alt);
  border: 1px solid var(--color-border-light);
  border-radius: 0.625rem;
  padding: 1rem;
}
.detail-head { display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.375rem; flex-wrap: wrap; }
.detail-node-dot { width: 0.625rem; height: 0.625rem; border-radius: 50%; flex-shrink: 0; }
.detail-node-name { font-size: 0.875rem; font-weight: 600; color: var(--color-text-primary); }
.detail-arrow { font-size: 0.875rem; color: var(--color-text-muted); }
.detail-path { font-size: 0.625rem; color: var(--color-text-muted); font-family: ui-monospace, monospace; margin-bottom: 0.875rem; word-break: break-all; }
.detail-empty { font-size: 0.75rem; color: var(--color-text-muted); }
.detail-edges { display: flex; flex-direction: column; gap: 0.5rem; }
.detail-edge-item { padding: 0.5rem; background: var(--color-surface); border: 1px solid var(--color-border); border-radius: 0.375rem; }
.detail-edge-row { display: flex; align-items: center; gap: 0.375rem; flex-wrap: wrap; }
.edge-dir { font-size: 0.75rem; color: var(--color-text-muted); font-weight: 600; }
.detail-edge-target { font-size: 0.75rem; color: var(--color-text-primary); font-weight: 500; flex: 1; }
.detail-evidence { font-size: 0.6875rem; color: var(--color-text-muted); margin-top: 0.375rem; line-height: 1.5; font-style: italic; }
.detail-ns-tag {
  font-size: 0.5625rem;
  color: var(--color-text-muted);
  margin-top: 0.25rem;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}
.del-btn {
  margin-left: auto;
  background: none;
  border: none;
  color: var(--color-text-muted);
  cursor: pointer;
  font-size: 1rem;
  line-height: 1;
  padding: 0 0.125rem;
  opacity: 0.5;
  transition: opacity 0.15s, color 0.15s;
}
.del-btn:hover { opacity: 1; color: var(--color-health-error, #e05c7c); }
.del-btn-full {
  margin-top: 0.75rem;
  font-size: 0.75rem;
  background: none;
  border: 1px solid var(--color-border);
  border-radius: 0.375rem;
  padding: 0.25rem 0.625rem;
  color: var(--color-text-muted);
  cursor: pointer;
  width: 100%;
  transition: color 0.15s, border-color 0.15s;
}
.del-btn-full:hover { color: var(--color-health-error, #e05c7c); border-color: var(--color-health-error, #e05c7c); }

.detail-hint { background: var(--color-surface-alt); border: 1px solid var(--color-border); border-radius: 0.625rem; padding: 1.25rem; text-align: center; }
.detail-hint p { font-size: 0.75rem; color: var(--color-text-muted); margin: 0 0 1rem; }
.graph-stats { display: flex; justify-content: center; gap: 2rem; }
.stat { display: flex; flex-direction: column; align-items: center; gap: 0.125rem; }
.stat-num { font-size: 1.5rem; font-weight: 700; color: var(--color-text-primary); font-variant-numeric: tabular-nums; letter-spacing: -0.02em; }
.stat-label { font-size: 0.625rem; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.05em; }

/* Relationship badges */
.rel-badge { font-size: 0.5625rem; padding: 0.0625rem 0.375rem; border-radius: 0.5rem; text-transform: uppercase; letter-spacing: 0.04em; font-weight: 600; white-space: nowrap; }
.rel-calls { color: var(--color-accent); background: color-mix(in srgb, var(--color-accent) 12%, transparent); }
.rel-imports { color: var(--color-type-project); background: color-mix(in srgb, var(--color-type-project) 12%, transparent); }
.rel-shares-schema { color: var(--color-health-warning); background: color-mix(in srgb, var(--color-health-warning) 12%, transparent); }
.rel-deploys-to { color: var(--color-type-reference); background: color-mix(in srgb, var(--color-type-reference) 12%, transparent); }
.rel-extends { color: var(--color-type-feedback); background: color-mix(in srgb, var(--color-type-feedback) 12%, transparent); }

/* Modal */
.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.55);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
  padding: 1rem;
}
.modal {
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 0.75rem;
  width: 100%;
  max-width: 28rem;
  max-height: 90vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.modal-wide { max-width: 42rem; }
.modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 1rem 1.25rem 0.75rem;
  border-bottom: 1px solid var(--color-border);
}
.modal-title { font-size: 1rem; font-weight: 600; color: var(--color-text-primary); margin: 0; }
.modal-close { background: none; border: none; font-size: 1.25rem; color: var(--color-text-muted); cursor: pointer; line-height: 1; padding: 0; }
.modal-close:hover { color: var(--color-text-primary); }
.modal-body { padding: 1rem 1.25rem; overflow-y: auto; flex: 1; display: flex; flex-direction: column; gap: 0.75rem; }
.modal-footer { padding: 0.75rem 1.25rem 1rem; border-top: 1px solid var(--color-border); display: flex; justify-content: flex-end; gap: 0.5rem; }

/* Form */
.form-row { display: flex; flex-direction: column; gap: 0.25rem; }
.form-label { font-size: 0.75rem; font-weight: 500; color: var(--color-text-secondary); }
.form-optional { font-weight: 400; color: var(--color-text-muted); }
.form-input {
  padding: 0.4375rem 0.625rem;
  background: var(--color-surface-alt);
  border: 1px solid var(--color-border);
  border-radius: 0.375rem;
  color: var(--color-text-primary);
  font-size: 0.8125rem;
  width: 100%;
  box-sizing: border-box;
  transition: border-color 0.15s;
}
.form-input:focus { outline: none; border-color: var(--color-accent); }
.form-input-inline { width: auto; flex: 1; }
.form-error { font-size: 0.75rem; color: var(--color-health-error, #e05c7c); margin: 0; }

/* Scan wizard */
.scan-intro { font-size: 0.8125rem; color: var(--color-text-muted); line-height: 1.6; margin: 0; }
.scan-ns-row { display: flex; align-items: center; gap: 0.75rem; }
.scan-ns-row .form-label { white-space: nowrap; margin: 0; }
.scan-pick-btn { align-self: flex-start; }
.scan-proposals { display: flex; flex-direction: column; gap: 0.5rem; }
.scan-proposals-header { display: flex; align-items: center; justify-content: space-between; }
.scan-proposals-count { font-size: 0.75rem; font-weight: 500; color: var(--color-text-secondary); }
.proposals-list { display: flex; flex-direction: column; gap: 0.375rem; max-height: 18rem; overflow-y: auto; }
.proposal-item {
  display: flex;
  align-items: flex-start;
  gap: 0.625rem;
  padding: 0.5rem 0.625rem;
  border: 1px solid var(--color-border);
  border-radius: 0.375rem;
  background: var(--color-surface-alt);
  cursor: pointer;
  transition: border-color 0.15s;
}
.proposal-item.is-checked { border-color: var(--color-accent); }
.proposal-checkbox { margin-top: 0.125rem; flex-shrink: 0; accent-color: var(--color-accent); }
.proposal-body { display: flex; flex-direction: column; gap: 0.25rem; min-width: 0; }
.proposal-rel { display: flex; align-items: center; gap: 0.375rem; flex-wrap: wrap; }
.proposal-repo { font-size: 0.75rem; font-weight: 500; color: var(--color-text-primary); }
.proposal-evidence { font-size: 0.6875rem; color: var(--color-text-muted); font-style: italic; }

.scan-done { align-items: center; text-align: center; padding: 2rem; }
.scan-done-icon { font-size: 2.5rem; color: var(--color-health-good, #56c97b); }
.scan-done-title { font-size: 1rem; font-weight: 500; color: var(--color-text-primary); margin: 0 0 1rem; }
</style>
