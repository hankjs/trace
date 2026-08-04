<script setup lang="ts">
import { watch } from "vue";
import { useRouter } from "vue-router";
import { useSession } from "./composables/useSession";
import { initTermNotifyListener } from "./api/termNotify";
import MessageToast from "./components/MessageToast.vue";

const { isAuthenticated } = useSession();
const router = useRouter();

// 终端通知全局监听（Rust 侧统一捕获，含无头终端），App 生命周期内注册一次
initTermNotifyListener();

watch(isAuthenticated, (authed) => {
  if (!authed) router.push({ name: "login" });
}, { immediate: true });
</script>

<template>
  <div class="h-screen flex flex-col">
    <MessageToast />
    <router-view class="flex-1 overflow-hidden" />
  </div>
</template>
