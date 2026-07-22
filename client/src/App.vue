<script setup lang="ts">
import { watch } from "vue";
import { useRouter } from "vue-router";
import { useSession } from "./composables/useSession";
import { useRemoteExec } from "./composables/useRemoteExec";
import { initTermNotifyListener } from "./api/termNotify";
import MessageToast from "./components/MessageToast.vue";

const { isAuthenticated } = useSession();
const remoteExec = useRemoteExec();
const router = useRouter();

// 终端通知全局监听（Rust 侧统一捕获，含无头终端），App 生命周期内注册一次
initTermNotifyListener(() => remoteExec.clientId);

watch(isAuthenticated, (authed) => {
  if (!authed) {
    remoteExec.stopPolling();
    router.push({ name: "login" });
    return;
  }
  // 登录后（含带 token 冷启动）：若已开启远程执行则注册并启动长轮询
  remoteExec.startIfEnabled();
}, { immediate: true });
</script>

<template>
  <div class="h-screen flex flex-col">
    <MessageToast />
    <router-view class="flex-1 overflow-hidden" />
  </div>
</template>
