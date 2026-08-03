<script setup lang="ts">
withDefaults(
  defineProps<{
    size?: number | string;
  }>(),
  { size: 28 },
);
</script>

<template>
  <!-- Inline mark: amber bot on transparent (inherits no bg for dark/light shell) -->
  <svg
    class="trace-logo"
    :width="size"
    :height="size"
    viewBox="0 0 64 64"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <!-- Antenna -->
    <rect x="30.5" y="4" width="3" height="9" rx="1.5" fill="currentColor" />
    <circle cx="32" cy="4.5" r="3" fill="currentColor" />
    <circle cx="32" cy="4.5" r="1.25" fill="var(--color-surface-0, #1f2124)" />

    <!-- Head shell -->
    <rect x="12" y="14" width="40" height="36" rx="10" fill="currentColor" />
    <!-- Face plate -->
    <rect x="16.5" y="18.5" width="31" height="27" rx="7" fill="var(--color-surface-0, #1f2124)" />

    <!-- Eyes -->
    <rect x="21.5" y="24" width="6" height="10.5" rx="3" fill="currentColor" />
    <rect x="36.5" y="24" width="6" height="10.5" rx="3" fill="currentColor" />

    <!-- Status bar -->
    <rect x="23" y="38.5" width="18" height="2.5" rx="1.25" fill="currentColor" />

    <!-- Side hinges -->
    <rect x="8.5" y="26" width="3.5" height="11" rx="1.5" fill="currentColor" />
    <rect x="52" y="26" width="3.5" height="11" rx="1.5" fill="currentColor" />

    <!-- Neck + base -->
    <rect x="27" y="48.5" width="10" height="4" rx="1.5" fill="currentColor" />
    <rect x="21" y="52" width="22" height="3.5" rx="1.5" fill="currentColor" />
  </svg>
</template>

<style scoped>
.trace-logo {
  display: block;
  color: var(--color-accent, #d4a24e);
  flex-shrink: 0;
}
</style>
