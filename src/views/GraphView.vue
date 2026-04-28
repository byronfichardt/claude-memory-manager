<script setup lang="ts">
import { onMounted, ref, computed, onUnmounted } from "vue";
import { useTauri } from "@/composables/useTauri";
import { useAppStore } from "@/stores/app";
import type { RepoGraph, RepoEdge } from "@/types";

const tauri = useTauri();
const app = useAppStore();

const graph = ref<RepoGraph | null>(null);
const loading = ref(true);
const selectedNode = ref<string | null>(null);
const selectedEdge = ref<RepoEdge | null>(null);

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

const nodes = computed<NodePos[]>(() => {
  if (!graph.value || graph.value.nodes.length === 0) return [];
  const n = graph.value.nodes.length;

  if (n === 1) {
    return [{ id: graph.value.nodes[0], x: CX, y: CY, label: shortName(graph.value.nodes[0]), edgeCount: 0 }];
  }

  return graph.value.nodes.map((id, i) => {
    const angle = (2 * Math.PI * i) / n - Math.PI / 2;
    const r = n <= 4 ? RADIUS * 0.7 : RADIUS;
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
  if (!graph.value) return 0;
  return graph.value.edges.filter(
    (e) => e.source_repo === id || e.target_repo === id,
  ).length;
}

function shortName(path: string): string {
  const parts = path.replace(/\\/g, "/").split("/");
  return parts[parts.length - 1] || path;
}

function edgePath(edge: RepoEdge): string {
  const s = nodeMap.value[edge.source_repo];
  const t = nodeMap.value[edge.target_repo];
  if (!s || !t) return "";

  // Offset slightly for bidirectional edges
  const dx = t.x - s.x;
  const dy = t.y - s.y;
  const len = Math.sqrt(dx * dx + dy * dy);
  if (len === 0) return "";

  // Perpendicular offset for curved edges
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
  return graph.value.edges.filter(
    (e) =>
      e.source_repo === selectedNode.value || e.target_repo === selectedNode.value,
  );
});

async function load() {
  loading.value = true;
  try {
    graph.value = await tauri.getRepoGraph();
  } catch (e) {
    app.error = String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(load);
onUnmounted(() => {});
</script>

<template>
  <div class="graph-view">
    <div class="graph-header">
      <h1 class="graph-title">Repo Relationship Graph</h1>
      <p class="graph-subtitle">
        Built organically as Claude notices inter-service dependencies.
        Claude will call <code>repo_link</code> when it discovers a dependency.
      </p>
    </div>

    <div v-if="loading" class="empty">Loading graph...</div>

    <div v-else-if="!graph || graph.nodes.length === 0" class="empty-state">
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
        As you work across projects, Claude will notice service dependencies
        (HTTP clients, imported SDKs, shared schemas) and call
        <code>repo_link</code> automatically. The graph builds over sessions.
      </p>
    </div>

    <div v-else class="graph-layout">
      <div class="graph-canvas-wrap">
        <svg
          class="graph-svg"
          :viewBox="`0 0 ${SVG_W} ${SVG_H}`"
          @click.self="clearSelection"
        >
          <defs>
            <marker
              id="arrowhead"
              markerWidth="8"
              markerHeight="6"
              refX="8"
              refY="3"
              orient="auto"
            >
              <polygon points="0 0, 8 3, 0 6" fill="var(--color-text-muted)" opacity="0.6" />
            </marker>
            <marker
              id="arrowhead-active"
              markerWidth="8"
              markerHeight="6"
              refX="8"
              refY="3"
              orient="auto"
            >
              <polygon points="0 0, 8 3, 0 6" fill="var(--color-accent)" />
            </marker>
          </defs>

          <!-- Edges -->
          <g class="edges">
            <path
              v-for="edge in graph.edges"
              :key="edge.id"
              :d="edgePath(edge)"
              class="edge-path"
              :class="{ 'is-active': isSelectedEdge(edge) }"
              fill="none"
              :marker-end="isSelectedEdge(edge) ? 'url(#arrowhead-active)' : 'url(#arrowhead)'"
              @click.stop="selectEdge(edge)"
            />
          </g>

          <!-- Nodes -->
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
            <text
              :x="node.x"
              :y="node.y + 4"
              text-anchor="middle"
              class="node-label"
            >
              {{ node.label.length > 12 ? node.label.slice(0, 11) + "…" : node.label }}
            </text>
            <text
              v-if="node.edgeCount > 0"
              :x="node.x + NODE_R - 4"
              :y="node.y - NODE_R + 8"
              text-anchor="middle"
              class="node-badge"
            >
              {{ node.edgeCount }}
            </text>
          </g>
        </svg>
      </div>

      <!-- Detail panel -->
      <div class="detail-panel">
        <!-- Node detail -->
        <div v-if="selectedNode" class="detail-card">
          <div class="detail-head">
            <div
              class="detail-node-dot"
              :style="{ background: nodeColor(selectedNode) }"
            ></div>
            <span class="detail-node-name">{{ shortName(selectedNode) }}</span>
          </div>
          <div class="detail-path">{{ selectedNode }}</div>

          <div v-if="selectedNodeEdges.length === 0" class="detail-empty">
            No recorded relationships.
          </div>
          <div v-else class="detail-edges">
            <div
              v-for="edge in selectedNodeEdges"
              :key="edge.id"
              class="detail-edge-item"
            >
              <div class="detail-edge-row">
                <span class="edge-dir">
                  {{ edge.source_repo === selectedNode ? "calls" : "called by" }}
                </span>
                <span
                  class="rel-badge"
                  :class="`rel-${edge.relationship_type}`"
                >{{ edge.relationship_type }}</span>
                <span class="detail-edge-target">
                  {{
                    shortName(
                      edge.source_repo === selectedNode
                        ? edge.target_repo
                        : edge.source_repo,
                    )
                  }}
                </span>
              </div>
              <div v-if="edge.evidence" class="detail-evidence">{{ edge.evidence }}</div>
            </div>
          </div>
        </div>

        <!-- Edge detail -->
        <div v-else-if="selectedEdge" class="detail-card">
          <div class="detail-head">
            <span class="detail-node-name">{{ shortName(selectedEdge.source_repo) }}</span>
            <span class="detail-arrow">→</span>
            <span class="detail-node-name">{{ shortName(selectedEdge.target_repo) }}</span>
          </div>
          <span class="rel-badge" :class="`rel-${selectedEdge.relationship_type}`">
            {{ selectedEdge.relationship_type }}
          </span>
          <div v-if="selectedEdge.evidence" class="detail-evidence" style="margin-top: 0.75rem">
            {{ selectedEdge.evidence }}
          </div>
        </div>

        <!-- Empty state -->
        <div v-else class="detail-hint">
          <p>Click a node or edge for details.</p>
          <div class="graph-stats">
            <div class="stat">
              <span class="stat-num">{{ graph.nodes.length }}</span>
              <span class="stat-label">repos</span>
            </div>
            <div class="stat">
              <span class="stat-num">{{ graph.edges.length }}</span>
              <span class="stat-label">dependencies</span>
            </div>
          </div>
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
.empty-icon {
  width: 4rem;
  height: 4rem;
  color: var(--color-text-muted);
  opacity: 0.4;
}
.empty-title {
  font-size: 1rem;
  font-weight: 500;
  color: var(--color-text-secondary);
  margin: 0;
}
.empty-hint {
  font-size: 0.8125rem;
  color: var(--color-text-muted);
  max-width: 28rem;
  line-height: 1.6;
  margin: 0;
}
.empty-hint code {
  font-family: ui-monospace, monospace;
  font-size: 0.75rem;
  color: var(--color-accent);
}

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
.graph-svg {
  width: 100%;
  height: auto;
  display: block;
  cursor: default;
}

.edge-path {
  stroke: var(--color-text-muted);
  stroke-width: 1.5;
  opacity: 0.35;
  cursor: pointer;
  transition: opacity 0.15s, stroke 0.15s;
}
.edge-path:hover,
.edge-path.is-active {
  stroke: var(--color-accent);
  opacity: 1;
  stroke-width: 2;
}

.node-group {
  cursor: pointer;
}
.node-circle {
  transition: opacity 0.2s, filter 0.2s;
}
.node-group:hover .node-circle,
.node-group.is-selected .node-circle {
  filter: brightness(1.15) drop-shadow(0 0 6px currentColor);
  opacity: 1 !important;
}
.node-label {
  font-size: 0.6875rem;
  font-weight: 600;
  fill: white;
  pointer-events: none;
  user-select: none;
}
.node-badge {
  font-size: 0.5625rem;
  font-weight: 700;
  fill: white;
  pointer-events: none;
  background: rgba(0, 0, 0, 0.4);
}

.detail-panel {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
}
.detail-card {
  background: var(--color-surface-alt);
  border: 1px solid var(--color-border-light);
  border-radius: 0.625rem;
  padding: 1rem;
}
.detail-head {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  margin-bottom: 0.375rem;
}
.detail-node-dot {
  width: 0.625rem;
  height: 0.625rem;
  border-radius: 50%;
  flex-shrink: 0;
}
.detail-node-name {
  font-size: 0.875rem;
  font-weight: 600;
  color: var(--color-text-primary);
}
.detail-arrow {
  font-size: 0.875rem;
  color: var(--color-text-muted);
}
.detail-path {
  font-size: 0.625rem;
  color: var(--color-text-muted);
  font-family: ui-monospace, monospace;
  margin-bottom: 0.875rem;
  word-break: break-all;
}
.detail-empty {
  font-size: 0.75rem;
  color: var(--color-text-muted);
}
.detail-edges {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}
.detail-edge-item {
  padding: 0.5rem;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 0.375rem;
}
.detail-edge-row {
  display: flex;
  align-items: center;
  gap: 0.375rem;
  flex-wrap: wrap;
}
.edge-dir {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
  font-style: italic;
}
.detail-edge-target {
  font-size: 0.75rem;
  color: var(--color-text-primary);
  font-weight: 500;
}
.detail-evidence {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
  margin-top: 0.375rem;
  line-height: 1.5;
  font-style: italic;
}

.rel-badge {
  font-size: 0.5625rem;
  padding: 0.0625rem 0.375rem;
  border-radius: 0.5rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  font-weight: 600;
  white-space: nowrap;
}
.rel-calls {
  color: var(--color-accent);
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
}
.rel-imports {
  color: var(--color-type-project);
  background: color-mix(in srgb, var(--color-type-project) 12%, transparent);
}
.rel-shares-schema {
  color: var(--color-health-warning);
  background: color-mix(in srgb, var(--color-health-warning) 12%, transparent);
}
.rel-deploys-to {
  color: var(--color-type-reference);
  background: color-mix(in srgb, var(--color-type-reference) 12%, transparent);
}
.rel-extends {
  color: var(--color-type-feedback);
  background: color-mix(in srgb, var(--color-type-feedback) 12%, transparent);
}

.detail-hint {
  background: var(--color-surface-alt);
  border: 1px solid var(--color-border);
  border-radius: 0.625rem;
  padding: 1.25rem;
  text-align: center;
}
.detail-hint p {
  font-size: 0.75rem;
  color: var(--color-text-muted);
  margin: 0 0 1rem;
}
.graph-stats {
  display: flex;
  justify-content: center;
  gap: 2rem;
}
.stat {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.125rem;
}
.stat-num {
  font-size: 1.5rem;
  font-weight: 700;
  color: var(--color-text-primary);
  font-variant-numeric: tabular-nums;
  letter-spacing: -0.02em;
}
.stat-label {
  font-size: 0.625rem;
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
}
</style>
