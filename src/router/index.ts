import { createRouter, createWebHashHistory } from "vue-router";

const routes = [
  {
    path: "/",
    name: "home",
    component: () => import("@/views/HomeView.vue"),
  },
  {
    path: "/topic/:name",
    name: "topic",
    component: () => import("@/views/TopicView.vue"),
    props: true,
  },
  {
    path: "/search",
    name: "search",
    component: () => import("@/views/SearchView.vue"),
  },
  {
    path: "/settings",
    name: "settings",
    component: () => import("@/views/SettingsView.vue"),
  },
  {
    path: "/graph",
    name: "graph",
    component: () => import("@/views/GraphView.vue"),
  },
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});
