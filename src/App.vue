<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter, useRoute } from "vue-router";
import { useAppStore } from "@/stores/app";

const router = useRouter();
const route = useRoute();
const app = useAppStore();

const searchInput = ref("");

onMounted(() => {
  app.initialize();
});

onUnmounted(() => {
  app.stopPolling();
});

watch(
  () => route.query.q,
  (q) => {
    if (typeof q === "string") searchInput.value = q;
  },
  { immediate: true },
);

function goHome() {
  searchInput.value = "";
  router.push({ name: "home" });
}

function goSettings() {
  router.push({ name: "settings" });
}

function goGraph() {
  router.push({ name: "graph" });
}

function onSearchSubmit() {
  const q = searchInput.value.trim();
  if (!q) return;
  router.push({ name: "search", query: { q } });
}

function onSearchClear() {
  searchInput.value = "";
  if (route.name === "search") router.push({ name: "home" });
}
</script>

<template>
  <div class="app-shell">
    <!-- Top bar -->
    <header class="topbar">
      <button class="brand" @click="goHome">
        <div class="brand-dot"></div>
        <span class="brand-name">Memory</span>
      </button>

      <form class="searchbar" @submit.prevent="onSearchSubmit">
        <svg class="search-icon" viewBox="0 0 16 16" fill="currentColor">
          <path fill-rule="evenodd" d="M9.965 11.026a5 5 0 111.06-1.06l2.755 2.754a.75.75 0 11-1.06 1.06l-2.755-2.754zM10.5 7a3.5 3.5 0 11-7 0 3.5 3.5 0 017 0z" clip-rule="evenodd" />
        </svg>
        <input
          v-model="searchInput"
          type="text"
          placeholder="Search memories..."
          class="search-input"
        />
        <button
          v-if="searchInput"
          type="button"
          class="search-clear"
          @click="onSearchClear"
        >
          ×
        </button>
      </form>

      <button
        class="icon-btn"
        :class="{ 'is-active': route.name === 'graph' }"
        title="Repo Graph"
        @click="goGraph"
      >
        <svg viewBox="0 0 16 16" fill="currentColor" class="icon">
          <circle cx="3.5" cy="8" r="2" />
          <circle cx="12.5" cy="3" r="2" />
          <circle cx="12.5" cy="13" r="2" />
          <line x1="5.4" y1="7.1" x2="10.6" y2="4.1" stroke="currentColor" stroke-width="1.25" />
          <line x1="5.4" y1="8.9" x2="10.6" y2="11.9" stroke="currentColor" stroke-width="1.25" />
        </svg>
      </button>

      <button
        class="icon-btn"
        :class="{ 'is-active': route.name === 'settings' }"
        title="Settings"
        @click="goSettings"
      >
        <svg viewBox="0 0 16 16" fill="currentColor" class="icon">
          <path fill-rule="evenodd" d="M6.955 1.45A.5.5 0 017.452 1h1.096a.5.5 0 01.497.45l.17 1.699c.484.12.94.312 1.356.562l1.321-.816a.5.5 0 01.67.087l.775.775a.5.5 0 01.087.67l-.815 1.32c.25.417.443.873.563 1.357l1.699.17a.5.5 0 01.45.497v1.096a.5.5 0 01-.45.497l-1.699.17c-.12.484-.312.94-.562 1.356l.816 1.321a.5.5 0 01-.087.67l-.775.775a.5.5 0 01-.67.087l-1.32-.815c-.417.25-.873.443-1.357.563l-.17 1.699a.5.5 0 01-.497.45H7.452a.5.5 0 01-.497-.45l-.17-1.699a4.973 4.973 0 01-1.356-.562l-1.321.816a.5.5 0 01-.67-.087l-.775-.775a.5.5 0 01-.087-.67l.816-1.32a4.973 4.973 0 01-.563-1.357l-1.699-.17A.5.5 0 011 8.548V7.452a.5.5 0 01.45-.497l1.699-.17c.12-.484.312-.94.562-1.356l-.816-1.321a.5.5 0 01.087-.67l.775-.775a.5.5 0 01.67-.087l1.32.816c.417-.25.873-.443 1.357-.563l.17-1.699zM8 10a2 2 0 100-4 2 2 0 000 4z" clip-rule="evenodd" />
        </svg>
      </button>
    </header>

    <main class="content">
      <router-view />
    </main>
  </div>
</template>

<style scoped>
.app-shell {
  display: flex;
  flex-direction: column;
  height: 100vh;
  width: 100vw;
  overflow: hidden;
}

.topbar {
  display: flex;
  align-items: center;
  gap: 1rem;
  height: 3rem;
  padding: 0 1.25rem;
  border-bottom: 1px solid var(--color-border);
  background: var(--color-surface-alt);
  flex-shrink: 0;
}

.brand {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  background: none;
  border: none;
  cursor: pointer;
  padding: 0.25rem 0;
  color: var(--color-text-primary);
  font-size: 0.875rem;
  font-weight: 600;
  letter-spacing: -0.01em;
}
.brand-dot {
  width: 0.5rem;
  height: 0.5rem;
  border-radius: 50%;
  background: var(--color-accent);
  box-shadow: 0 0 10px color-mix(in srgb, var(--color-accent) 60%, transparent);
}

.searchbar {
  flex: 1;
  max-width: 32rem;
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.375rem 0.75rem;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 0.5rem;
  transition: border-color 0.15s;
}
.searchbar:focus-within {
  border-color: var(--color-accent-muted);
}
.search-icon {
  width: 0.875rem;
  height: 0.875rem;
  color: var(--color-text-muted);
  flex-shrink: 0;
}
.search-input {
  flex: 1;
  background: transparent;
  border: none;
  outline: none;
  font-size: 0.8125rem;
  color: var(--color-text-primary);
  min-width: 0;
}
.search-input::placeholder {
  color: var(--color-text-muted);
}
.search-clear {
  width: 1rem;
  height: 1rem;
  background: none;
  border: none;
  cursor: pointer;
  color: var(--color-text-muted);
  font-size: 1.25rem;
  line-height: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0;
}
.search-clear:hover {
  color: var(--color-text-primary);
}

.icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 2rem;
  height: 2rem;
  border: 1px solid transparent;
  border-radius: 0.375rem;
  background: none;
  cursor: pointer;
  color: var(--color-text-muted);
}
.icon-btn:hover {
  background: var(--color-surface-hover);
  color: var(--color-text-primary);
}
.icon-btn.is-active {
  color: var(--color-accent);
  border-color: color-mix(in srgb, var(--color-accent) 30%, transparent);
  background: color-mix(in srgb, var(--color-accent) 10%, transparent);
}
.icon {
  width: 1rem;
  height: 1rem;
}

.content {
  flex: 1;
  overflow-y: auto;
}
</style>
