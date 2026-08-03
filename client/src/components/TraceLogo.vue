<script setup lang="ts">
/**
 * Trace 品牌 mark（对齐 assets/brand/trace-bot-logo：圆角 bot + 琥珀轨迹）。
 * 用 currentColor 继承 accent，适配深浅壳；app 图标走 Tauri icons 位图。
 */
withDefaults(
  defineProps<{
    size?: number | string;
  }>(),
  { size: 28 },
);
</script>

<template>
  <svg
    class="trace-logo"
    :width="size"
    :height="size"
    viewBox="0 0 64 64"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <!-- 轨道轨迹（trace） -->
    <path
      d="M10 44C14 22 28 10 44 12C52 13 58 18 58 28"
      stroke="currentColor"
      stroke-width="1.75"
      stroke-linecap="round"
      stroke-dasharray="2.5 3.5"
      opacity="0.55"
    />
    <path
      d="M54 20C50 42 36 54 20 52C12 51 6 46 6 36"
      stroke="currentColor"
      stroke-width="1.75"
      stroke-linecap="round"
      stroke-dasharray="2.5 3.5"
      opacity="0.35"
    />
    <circle cx="44" cy="12" r="1.6" fill="currentColor" opacity="0.7" />
    <circle cx="58" cy="28" r="1.4" fill="currentColor" opacity="0.55" />
    <circle cx="20" cy="52" r="1.4" fill="currentColor" opacity="0.45" />

    <!-- 头壳 -->
    <rect x="14" y="16" width="36" height="34" rx="10" fill="currentColor" />
    <!-- 脸板 -->
    <rect
      x="18"
      y="20"
      width="28"
      height="26"
      rx="7"
      fill="var(--color-surface-0, #1f2124)"
    />

    <!-- 圆形传感器眼 -->
    <circle cx="26" cy="31" r="3.2" fill="currentColor" />
    <circle cx="38" cy="31" r="3.2" fill="currentColor" />
    <circle cx="25.2" cy="30.2" r="1" fill="var(--color-surface-0, #1f2124)" opacity="0.55" />
    <circle cx="37.2" cy="30.2" r="1" fill="var(--color-surface-0, #1f2124)" opacity="0.55" />

    <!-- 状态条（口） -->
    <rect x="24" y="39.5" width="16" height="2.6" rx="1.3" fill="currentColor" />
  </svg>
</template>

<style scoped>
.trace-logo {
  display: block;
  color: var(--color-accent, #d4a24e);
  flex-shrink: 0;
}
</style>
