<script setup lang="ts">
import { watch } from "vue";
import { useRouter } from "vue-router";
import { useSession } from "./composables/useSession";
import { useRemoteTerm } from "./composables/useRemoteTerm";
import { initTermNotifyListener } from "./api/termNotify";
import MessageToast from "./components/MessageToast.vue";

const { isAuthenticated } = useSession();
const remoteTerm = useRemoteTerm();
const router = useRouter();

// 终端通知全局监听（Rust 侧统一捕获，含无头终端），App 生命周期内注册一次
initTermNotifyListener();

watch(isAuthenticated, (authed) => {
  if (!authed) {
    remoteTerm.stopPolling();
    router.push({ name: "login" });
    return;
  }
  // 登录后（含带 token 冷启动）：若已开启远程终端则注册并启动长轮询
  // 不 await：避免挡住路由；失败时设置页会显示 lastError
  void remoteTerm.startIfEnabled();
}, { immediate: true });
</script>

<template>
  <div class="h-screen flex flex-col">
    <MessageToast />
    <router-view class="flex-1 overflow-hidden" />
  </div>
</template>
