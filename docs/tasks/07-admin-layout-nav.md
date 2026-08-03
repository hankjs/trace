# 任务 07：整理 admin 菜单 + Layout 独立滚动

## 背景与目标

`admin/` 后台目前存在两个问题：

1. **菜单是一条 16 项的平铺列表**，没有任何分组，图标混用了 emoji（`🖼`）和几何字符（`◫ ☰ ⊙ ✎ ⚡`），视觉上杂乱、扫读困难。
2. **Layout 没有独立滚动**：根容器用 `min-h-screen`，整页作为一个文档流滚动。侧边栏高度只跟随内容，撑不满屏；内容区变长时侧边栏被一起滚走。`<main>` 自带 `px-10 py-8` 内边距，却又对 `/terminals` 单独叠加 `h-screen overflow-hidden`，导致 padding 被重复计入高度、内容溢出视口。

做完之后的可观察效果：

- 左侧菜单按业务域分组，组内带小标题；图标统一为一套线性 SVG（Lucide 风格，`stroke-width=1.75`），不再有 emoji。
- 侧边栏固定满屏高度，不随内容区滚动；菜单项过多时侧边栏自己内部滚动，品牌标与「退出登录」始终可见。
- 内容区独立滚动，滚动条贴在视口右边缘。
- `/terminals`、`/chat-records` 这类"占满高度"的页面不再溢出，内部区域自己滚动。
- 页面宽度与是否满高不再靠 `App.vue` 里的 `route.path === '...'` 硬编码判断，改为路由 `meta` 声明。

## 涉及文件清单

| 文件 | 改动 |
|------|------|
| `admin/src/components/NavIcon.vue` | **新建**，按 `name` 渲染内联 SVG 图标 |
| `admin/src/App.vue` | 重写 Layout 骨架 + 菜单改为分组结构 + 移除路径硬编码 |
| `admin/src/main.ts` | 给路由补 `meta`（`fill` / `width`） |
| `admin/src/style.css` | 追加细滚动条样式 |
| `CLAUDE.md` | Admin 页面章节补一句菜单分组说明 |

## 实现步骤

### 1. 新建 `admin/src/components/NavIcon.vue`

统一的线性图标组件。24×24 viewBox，`fill=none` + `currentColor` 描边，尺寸由外部 class 控制。

```vue
<script setup lang="ts">
// 菜单图标：Lucide 风格线性图标，按 name 取 path 片段
const props = defineProps<{ name: string }>()

// 每项是一组 SVG 子元素字符串，直接内联渲染
const icons: Record<string, string> = {
  dashboard: '<rect x="3" y="3" width="7" height="9" rx="1"/><rect x="14" y="3" width="7" height="5" rx="1"/><rect x="14" y="12" width="7" height="9" rx="1"/><rect x="3" y="16" width="7" height="5" rx="1"/>',
  // ... 见下表
}
</script>

<template>
  <svg
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.75"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    v-html="icons[props.name] ?? ''"
  />
</template>
```

需要的 16 个图标（`name` → SVG 子元素）：

- `dashboard`：见上方示例
- `sessions`：`<path d="M14 9a2 2 0 0 1-2 2H6l-4 4V4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2z"/><path d="M18 9h2a2 2 0 0 1 2 2v11l-4-4h-6a2 2 0 0 1-2-2v-1"/>`
- `explore`：`<circle cx="12" cy="12" r="10"/><polygon points="16.24 7.76 14.12 14.12 7.76 16.24 9.88 9.88 16.24 7.76"/>`
- `prompt`：`<path d="m12 3-1.9 5.8a2 2 0 0 1-1.287 1.288L3 12l5.8 1.9a2 2 0 0 1 1.288 1.287L12 21l1.9-5.8a2 2 0 0 1 1.287-1.288L21 12l-5.8-1.9a2 2 0 0 1-1.288-1.287z"/>`
- `provider`：`<path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z"/>`
- `image`：`<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>`
- `cli`：`<rect x="3" y="3" width="18" height="18" rx="2"/><path d="m7 11 2-2-2-2"/><path d="M11 13h4"/>`
- `weixin`：`<path d="M7.9 20A9 9 0 1 0 4 16.1L2 22z"/>`
- `feishu`：`<path d="M14.536 21.686a.5.5 0 0 0 .937-.024l6.5-19a.496.496 0 0 0-.635-.635l-19 6.5a.5.5 0 0 0-.024.937l7.93 3.18a2 2 0 0 1 1.112 1.11z"/><path d="m21.854 2.147-10.94 10.939"/>`
- `records`：`<rect x="2" y="3" width="20" height="5" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/><path d="M10 12h4"/>`
- `jobs`：`<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>`
- `interaction`：`<path d="M21.801 10A10 10 0 1 1 17 3.335"/><path d="m9 11 3 3L22 4"/>`
- `teamTask`：`<rect x="3" y="3" width="8" height="8" rx="2"/><path d="M7 11v4a2 2 0 0 0 2 2h4"/><rect x="13" y="13" width="8" height="8" rx="2"/>`
- `terminal`：`<polyline points="4 17 10 11 4 5"/><path d="M12 19h8"/>`
- `notification`：`<path d="M10.268 21a2 2 0 0 0 3.464 0"/><path d="M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326"/>`
- `users`：`<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>`

> `v-html` 内容全部是本文件内的常量字符串，不接受外部输入，无 XSS 风险。

### 2. `admin/src/App.vue`：菜单改为分组结构

把原来的 `nav` 平铺数组替换为分组数组，保持所有 `to` 路径不变、`label` 文案不变，只调整顺序与分组，并把 `icon` 换成 `NavIcon` 的 `name`：

```ts
const navGroups: { title?: string; items: { to: string; label: string; icon: string }[] }[] = [
  { items: [{ to: '/', label: '概览', icon: 'dashboard' }] },
  {
    title: '会话与追踪',
    items: [
      { to: '/sessions', label: '会话', icon: 'sessions' },
      { to: '/explore', label: '探索', icon: 'explore' },
    ],
  },
  {
    title: '渠道',
    items: [
      { to: '/feishu', label: '飞书机器人', icon: 'feishu' },
      { to: '/weixin', label: '微信机器人', icon: 'weixin' },
      { to: '/chat-records', label: '聊天记录', icon: 'records' },
      { to: '/notifications', label: '通知', icon: 'notification' },
    ],
  },
  {
    title: '任务',
    items: [
      { to: '/team-task', label: '团队任务', icon: 'teamTask' },
      { to: '/jobs', label: '定时任务', icon: 'jobs' },
      { to: '/interactions', label: '交互单', icon: 'interaction' },
    ],
  },
  {
    title: '模型与工具',
    items: [
      { to: '/providers', label: '供应商', icon: 'provider' },
      { to: '/image-providers', label: '生图供应商', icon: 'image' },
      { to: '/agent-cli', label: 'Agent CLI', icon: 'cli' },
      { to: '/prompts', label: '提示词', icon: 'prompt' },
    ],
  },
  {
    title: '系统',
    items: [
      { to: '/terminals', label: '终端', icon: 'terminal' },
      { to: '/users', label: '用户', icon: 'users' },
    ],
  },
]
```

`isActive` 逻辑保持不变。

### 3. `admin/src/App.vue`：重写 Layout 骨架

关键点：根容器改 `h-screen overflow-hidden`；侧边栏与内容区各自 `overflow-y-auto`；内边距移到内容区的内层 wrapper，让滚动条贴视口边缘。

```vue
<div v-else class="flex h-screen overflow-hidden">
  <aside class="flex h-full w-52 shrink-0 flex-col overflow-hidden border-r border-border-subtle">
    <!-- 品牌标：固定不滚动 -->
    <div class="flex shrink-0 items-center gap-2 px-5 py-5">
      <img :src="faviconUrl" alt="" width="20" height="20" class="shrink-0 rounded-[5px]" />
      <span class="text-sm font-medium tracking-tight text-text-secondary">Trace</span>
    </div>

    <!-- 菜单：侧边栏内部独立滚动 -->
    <nav class="thin-scrollbar min-h-0 flex-1 overflow-y-auto px-3 pb-2">
      <div v-for="(group, gi) in navGroups" :key="gi" :class="gi > 0 ? 'mt-4' : ''">
        <p
          v-if="group.title"
          class="px-2 pb-1 text-[11px] font-medium tracking-wide text-text-tertiary"
        >{{ group.title }}</p>
        <RouterLink
          v-for="item in group.items"
          :key="item.to"
          :to="item.to"
          class="flex items-center gap-2.5 rounded-md px-2 py-1.5 text-[13px] transition-colors duration-100"
          :class="isActive(item.to) ? 'bg-active text-text-primary font-medium' : 'text-text-secondary hover:bg-hover'"
        >
          <NavIcon :name="item.icon" class="size-4 shrink-0 opacity-70" />
          {{ item.label }}
        </RouterLink>
      </div>
    </nav>

    <!-- 退出登录：固定在底部 -->
    <div class="shrink-0 border-t border-border-subtle px-3 py-2">
      <button
        @click="logout"
        class="w-full px-2 py-1.5 text-left text-[12px] text-text-tertiary transition-colors hover:text-text-secondary"
      >退出登录</button>
    </div>
  </aside>

  <main
    class="min-w-0 flex-1"
    :class="isFill ? 'flex flex-col overflow-hidden' : 'thin-scrollbar overflow-y-auto'"
  >
    <div
      class="px-10 py-8"
      :class="[
        widthClass,
        isFill ? 'flex min-h-0 w-full flex-1 flex-col' : '',
      ]"
    >
      <RouterView />
    </div>
  </main>
</div>
```

配套的计算属性，替换掉原先 `route.path === '/terminals'` 那串三元判断：

```ts
// 是否"占满视口高度、内部自行滚动"的页面（由路由 meta.fill 声明）
const isFill = computed(() => route.meta.fill === true)
// 内容区最大宽度：meta.width 为 'wide' 时放宽，'full' 时不限
const widthClass = computed(() => {
  if (route.meta.width === 'full') return 'w-full'
  if (route.meta.width === 'wide') return 'w-full max-w-[1400px]'
  return 'max-w-4xl'
})
```

记得 `import { computed, ref } from 'vue'` 并 `import NavIcon from './components/NavIcon.vue'`。

### 4. `admin/src/main.ts`：补路由 meta

只加 `meta`，不动路径与组件：

- `/terminals` → `meta: { fill: true, width: 'full' }`
- `/chat-records` → `meta: { fill: true, width: 'wide' }`
- `/interactions`、`/interactions/:id` → `meta: { width: 'wide' }`
- `/explore/:id`、`/sessions/:id/explore` → `meta: { width: 'wide' }`（这两个页面内部已声明 `max-w-5xl`，外层不该再压到 `max-w-4xl`）

其余路由不加 meta，走默认 `max-w-4xl`。

为让 `route.meta.fill` 有类型，在 `main.ts` 顶部补一段模块声明：

```ts
declare module 'vue-router' {
  interface RouteMeta {
    public?: boolean
    fill?: boolean
    width?: 'wide' | 'full'
  }
}
```

### 5. `admin/src/style.css`：追加细滚动条

```css
/* 细滚动条：侧边栏与内容区独立滚动时不抢视觉重心 */
.thin-scrollbar {
  scrollbar-width: thin;
  scrollbar-color: var(--color-border) transparent;
}
.thin-scrollbar::-webkit-scrollbar {
  width: 8px;
  height: 8px;
}
.thin-scrollbar::-webkit-scrollbar-track {
  background: transparent;
}
.thin-scrollbar::-webkit-scrollbar-thumb {
  background: var(--color-border);
  border-radius: 4px;
}
.thin-scrollbar::-webkit-scrollbar-thumb:hover {
  background: var(--color-active);
}
```

### 6. `CLAUDE.md`

在「Admin 页面」章节的表格上方那句说明后面，补一句：

> 左侧菜单按业务域分组（概览 / 会话与追踪 / 渠道 / 任务 / 模型与工具 / 系统），图标统一走 `components/NavIcon.vue`；页面宽度与是否满高由路由 `meta.width`、`meta.fill` 声明，不要在 `App.vue` 里按路径硬编码。

## 明确边界

- **只改上表列出的 5 个文件 + CLAUDE.md**。不要改 `admin/src/views/**` 下任何页面组件，也不要改 `admin/src/components/` 下已有组件（只新增 `NavIcon.vue`）。
- 不动 `client/`、`team/`、`server/`、`crates/`、`quant/`。
- 不改路由路径、不改页面 `label` 文案、不删任何菜单项 —— 16 项一个不少。
- 不新增 npm 依赖（不要引入 lucide-vue-next 之类的图标库，图标手写内联 SVG）。
- 保留工作区已有的其他改动，不要 `git checkout` / `git stash` / 回退与本任务无关的内容。
- `main.ts` 里那段飞书深链 hash 改写逻辑保持原样。

## 验收标准

```bash
cd admin && pnpm install && pnpm build
```

- `vue-tsc` 零报错（`meta.fill` / `meta.width` 需要靠第 4 步的 `RouteMeta` 声明通过类型检查），vite 构建成功。
- `cd admin && pnpm dev` 后人工确认：
  1. 侧边栏边框从视口顶部贯通到底部；
  2. 内容区滚到底时，侧边栏与「退出登录」不动；
  3. 把窗口高度压到很小，侧边栏菜单区自己出现滚动条，品牌标与「退出登录」仍固定可见；
  4. `/terminals`、`/chat-records` 没有整页滚动条，内容不溢出视口；
  5. 菜单分组标题显示正确，16 项全在，图标无 emoji、粗细一致；
  6. 逐个点击菜单，高亮态与页面均正常。

## 约定

遵循 `CLAUDE.md`：`<script setup lang="ts">` + Composition API，样式用 Tailwind utility class，注释与 commit message 用中文。commit message 建议：`refactor(admin): 菜单分组与 Layout 独立滚动`。
