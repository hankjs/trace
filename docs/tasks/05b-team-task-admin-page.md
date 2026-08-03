# 任务 05b：团队任务配置 admin 页面

> 05a 的后续：05a 做了 DB 存储 + REST，本份做 admin 前端页面。
> **执行前提**：05a 已完成并验收通过（`/api/admin/team-task/config` 的 GET/PATCH 可用）。

## 背景与目标

05a 把两个开关（`task_gate_enabled` / `team_task.enabled`）与四个参数
（`roles` / `gates` / `max_dev_rounds` / `dashboard_base_url`）移到了 `settings` 表，
并提供了 REST。现在缺一个页面把它们暴露出来 —— 否则还得手搓 curl。

### 本任务目标

新增 admin「团队任务」页：读当前配置、改开关与参数、保存后即时生效。

### 做完之后的可观察效果

1. admin 侧栏出现「团队任务」入口，路由 `/team-task`。
2. 页面显示两个总开关（闸门 / 流水线）、角色顺序、闸门边界多选、
   最大返工轮次、看板地址。
3. 改完点保存 → 成功提示；**不需要重启 server**，下一个飞书任务按新配置走。
4. 提交非法组合（如只开流水线不开闸门）→ 页面显示后端返回的中文错误原因。
5. 当前还在用 `config.toml` 默认值时，页面有明确提示。

## 涉及文件清单

| 文件 | 改动 |
|------|------|
| `admin/src/views/TeamTask.vue` | **新建**。配置页 |
| `admin/src/composables/api.ts` | 加 `TeamTaskConfig` 类型与 `getTeamTaskConfig` / `updateTeamTaskConfig` |
| `admin/src/main.ts` | 加路由 `/team-task` |
| `admin/src/App.vue` | 侧栏加入口 |

**只碰 `admin/` 下这四个文件。**

## 实现步骤

### 步骤 1：API 客户端

- [ ] **1.1** 在 `admin/src/composables/api.ts` 加类型。字段与 05a 的
  `TeamTaskSettings` 及 GET 响应对齐：

```ts
export interface TeamTaskConfig {
  task_gate_enabled: boolean
  enabled: boolean
  roles: string[]
  gates: string[]
  max_dev_rounds: number
  dashboard_base_url: string | null
  updated_by: string | null
}

export interface TeamTaskOption {
  id: string
  label: string
}

export interface TeamTaskConfigResponse {
  config: TeamTaskConfig
  /** db = 已在 admin 改过；config_file = 还在用 config.toml 默认值 */
  source: 'db' | 'config_file'
  role_options: TeamTaskOption[]
  gate_options: TeamTaskOption[]
}
```

- [ ] **1.2** 加两个方法，沿用文件里现有的 `request` 封装：

```ts
getTeamTaskConfig() {
  return request<TeamTaskConfigResponse>('/api/admin/team-task/config')
},
updateTeamTaskConfig(data: Partial<TeamTaskConfig>) {
  return request<TeamTaskConfigResponse>('/api/admin/team-task/config', {
    method: 'PATCH',
    body: JSON.stringify(data),
  })
},
```

`updateTeamTaskConfig` 用 `Partial<>`，与 05a 的 PATCH 语义（只改传了的字段）对齐。

### 步骤 2：页面

- [ ] **2.1** 新建 `admin/src/views/TeamTask.vue`。风格与结构参考
  `admin/src/views/Jobs.vue`（`<script setup lang="ts">` + Tailwind utility class），
  文件头写中文块注释说明这个页面管什么、为什么改完不用重启。

- [ ] **2.2** 页面分区：

**① 两个总开关**（放最上面，最常用）
- 「两阶段闸门」`task_gate_enabled`：开启后飞书代码任务先只读分析，
  产出目标/范围/疑似改动点/风险，等你点「开始修」才真正改代码
- 「多角色流水线」`enabled`：开启后「开始修」之后按开发 → 评审 → 测试串行执行
- **依赖关系要在 UI 上体现**：`task_gate_enabled` 关闭时，
  流水线开关置灰并提示「需要先开启两阶段闸门」。这样用户在点保存之前就知道，
  而不是提交后吃一个 400

**② 角色顺序**（`roles`）
- 用 `role_options` 渲染。要能调顺序（上移/下移按钮即可，不必做拖拽）
  和勾选启用哪些
- 顺序有意义：流水线按数组顺序流转，所以 UI 要让顺序可见
- 至少勾一个，全不勾时保存按钮禁用并提示

**③ 闸门边界**（`gates`）多选
- 用 `gate_options` 渲染，每项显示后端给的中文 label
- 加一行说明：勾得越多，一个任务需要人工点的次数越多；
  全不勾即全自动流转

**④ 参数**
- `max_dev_rounds`：数字输入，范围 1–10（与 05a 的校验一致）。
  加说明「评审打回后最多重新开发几轮，超出即失败」
- `dashboard_base_url`：文本输入，可留空。说明「留空则飞书卡片不显示看板链接」

**⑤ 保存区**
- 保存按钮 + 结果提示（成功/失败）
- 失败时**显示后端返回的原文错误**（05a 的 `validate` 消息已经是面向用户的中文），
  不要自己另编一套文案 —— 两套文案会漂移

- [ ] **2.3** `source === 'config_file'` 时在页面顶部显示一条提示：
  「当前使用配置文件中的默认值，尚未在此页面保存过。保存后将以此处配置为准。」

- [ ] **2.4** 显示 `updated_by`（若有）：「最后修改人：xxx」。

- [ ] **2.5** 交互细节：
  - 初次加载 loading 态
  - 保存中禁用按钮，防重复提交
  - 保存成功后用响应里的 `config` 刷新本地状态（而不是乐观更新），
    这样后端做了归一时前端能看到真实值
  - **不要做轮询**。这是配置页，不是状态页；Jobs.vue 轮询是因为有运行中的任务

### 步骤 3：路由与侧栏

- [ ] **3.1** `admin/src/main.ts` 加路由，位置紧邻 `/jobs`：

```ts
{ path: '/team-task', component: () => import('./views/TeamTask.vue') },
```

- [ ] **3.2** `admin/src/App.vue` 的侧栏数组加一项。放在「交互单」之后
  （两者都属于任务流相关）：

```ts
{ to: '/team-task', label: '团队任务', icon: '⛓' },
```

图标从现有那批 Unicode 符号里挑一个不冲突的即可。

- [ ] **3.3** 检查 `App.vue` 里那个按路由判断样式的逻辑（约 73 行，
  `route.path === '/chat-records' || route.path.startsWith('/interactions')`）
  是否需要把新路由纳入 —— 按该处实际语义决定，不确定就不动。

## 明确边界

**不许碰**：
- `server/`、`crates/`、`client/`、`quant/`、`cli/`（后端 05a 已完成）
- `admin/` 下除上述四个文件外的任何文件
- `admin/package.json`（**不要新增前端依赖**，现有 Vue + Tailwind 够用）
- `config.toml`、`CLAUDE.md`、`docs/`

**不许做**：
- 不要改 REST 接口形状（那是 05a 的产出，已验收）
- 不要在前端硬编码角色名或闸门边界名 —— 用后端返回的
  `role_options` / `gate_options`，加第四个角色时前端不用改
- 不要自己写一套校验文案 —— 复用后端 `validate` 返回的消息。
  前端只做「明显不合法就禁用保存按钮」这种即时反馈（如全不勾角色）
- 不要做轮询
- 不要写团队任务看板（那是第 8 步，独立前端工程）

## 验收标准

```bash
cd admin && pnpm build    # 严格 TS 检查，必须零错误
```

期望结果：
- `pnpm build` 成功（`vue-tsc` 零错误）
- `git status` 只显示 `admin/` 下那四个文件

**手工验证**（需要 05a 的后端跑着）：
- [ ] `cd admin && pnpm dev` 打开页面，能看到当前配置
- [ ] 首次打开显示「使用配置文件默认值」提示
- [ ] 关闭「两阶段闸门」→ 流水线开关自动置灰并提示依赖
- [ ] 开启两个开关 → 保存成功 → 刷新页面配置保持，提示变为已保存状态
- [ ] 角色全不勾 → 保存按钮禁用
- [ ] `max_dev_rounds` 填 0 或 11 → 保存被拒（前端拦或后端 400 都可接受，
      但必须有可见反馈）
- [ ] 保存后**不重启 server**，飞书派一个代码任务，确认按新配置走

## 约定

遵循 `CLAUDE.md`：

- 前端用 `<script setup lang="ts">` + Composition API
- 样式用 Tailwind utility class，视觉风格与 `Jobs.vue` / `Interactions.vue` 一致
- **中文注释 + 中文界面文案**
- 两处必须注释写清「为什么」：
  1. 选项由后端返回而非前端硬编码 —— 加角色时只改一处
  2. 不做轮询 —— 配置页没有需要追踪的运行态
- **中文 commit message**，形如
  `feat(admin): 团队任务配置页，开关与参数在线可改`
- API 调用集中在 `composables/api.ts`，页面不直接 `fetch`
