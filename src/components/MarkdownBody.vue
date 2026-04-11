<script setup lang="ts">
import { computed } from "vue";
import { marked } from "marked";

const props = defineProps<{
  content: string;
}>();

marked.setOptions({
  gfm: true,
  breaks: true,
});

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

const rendered = computed(() => {
  if (!props.content) return "";
  try {
    return marked.parse(props.content, { async: false }) as string;
  } catch {
    return `<pre>${escapeHtml(props.content)}</pre>`;
  }
});
</script>

<template>
  <div class="markdown-body" v-html="rendered"></div>
</template>

<style scoped>
.markdown-body {
  color: var(--color-text-secondary);
  font-size: 0.8125rem;
  line-height: 1.65;
}
.markdown-body :deep(h1),
.markdown-body :deep(h2),
.markdown-body :deep(h3),
.markdown-body :deep(h4) {
  color: var(--color-text-primary);
  font-weight: 600;
  margin: 1.25em 0 0.5em;
  line-height: 1.3;
}
.markdown-body :deep(h1):first-child,
.markdown-body :deep(h2):first-child,
.markdown-body :deep(h3):first-child {
  margin-top: 0;
}
.markdown-body :deep(h1) {
  font-size: 1.25rem;
}
.markdown-body :deep(h2) {
  font-size: 1.0625rem;
}
.markdown-body :deep(h3) {
  font-size: 0.9375rem;
}
.markdown-body :deep(p) {
  margin: 0 0 0.75em;
}
.markdown-body :deep(p):last-child {
  margin-bottom: 0;
}
.markdown-body :deep(ul),
.markdown-body :deep(ol) {
  margin: 0 0 0.75em;
  padding-left: 1.25rem;
}
.markdown-body :deep(li) {
  margin-bottom: 0.25em;
}
.markdown-body :deep(code) {
  font-family: ui-monospace, "SF Mono", Menlo, monospace;
  font-size: 0.78em;
  background: var(--color-surface-alt);
  padding: 0.0625rem 0.375rem;
  border-radius: 0.25rem;
  color: var(--color-accent);
  border: 1px solid var(--color-border);
}
.markdown-body :deep(pre) {
  background: var(--color-surface-alt);
  border: 1px solid var(--color-border);
  border-radius: 0.375rem;
  padding: 0.75rem;
  overflow-x: auto;
  margin: 0 0 0.75em;
}
.markdown-body :deep(pre) :deep(code) {
  background: none;
  border: none;
  padding: 0;
  color: var(--color-text-primary);
  font-size: 0.75rem;
}
.markdown-body :deep(blockquote) {
  border-left: 2px solid var(--color-accent-muted);
  padding-left: 0.875rem;
  margin: 0 0 0.75em;
  color: var(--color-text-muted);
  font-style: italic;
}
.markdown-body :deep(a) {
  color: var(--color-accent);
  text-decoration: underline;
  text-decoration-color: color-mix(
    in srgb,
    var(--color-accent) 40%,
    transparent
  );
}
.markdown-body :deep(a):hover {
  text-decoration-color: var(--color-accent);
}
.markdown-body :deep(strong) {
  color: var(--color-text-primary);
  font-weight: 600;
}
.markdown-body :deep(em) {
  color: var(--color-text-primary);
}
.markdown-body :deep(hr) {
  border: none;
  border-top: 1px solid var(--color-border);
  margin: 1em 0;
}
.markdown-body :deep(table) {
  width: 100%;
  border-collapse: collapse;
  margin: 0 0 0.75em;
  font-size: 0.75rem;
}
.markdown-body :deep(th),
.markdown-body :deep(td) {
  padding: 0.375rem 0.625rem;
  border: 1px solid var(--color-border);
  text-align: left;
}
.markdown-body :deep(th) {
  background: var(--color-surface-alt);
  color: var(--color-text-primary);
  font-weight: 600;
}
</style>
