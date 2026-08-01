# Server Agent 双向 Git 同步协议

> 协议版本：3
>
> 本文件是本机、wananyun 和飞书 server Agent 共同维护的唯一同步流程。
> `AGENTS.md`、飞书指南和脚本只引用本文件，不复制另一套流程。

## 目标

- 飞书 Agent 在 wananyun 本地完成代码修改、提交、测试和审批部署；工作区由任务意图决定。
- wananyun 不依赖 GitHub 网络，不执行 GitHub `clone`、`fetch` 或 `push`。
- 本机通过 SSH 拉回 wananyun 的 Git 提交，人工检查后合并并手动 push GitHub。
- server 与本机共享同一段 Git 历史，不通过 rsync 覆盖开发工作区。
- 可迭代 `server/`、`crates/`、`admin/`、`cli/`、`quant/`、`docs/`；始终排除 `client/`。

## 唯一事实来源

| 状态 | 位置 | 含义 | 谁可以推进 |
|------|------|------|------------|
| `master` | 本机 / GitHub | 上游历史 | 本机人工 push |
| `trace-production` | wananyun `/opt/hank-src` | 当前生产代码基线 | 审批后的部署 helper，或 SSH 手工部署 |
| `feishu/<session-uuid>` | wananyun | 一个飞书话题的工作分支 | 对应话题 Agent |
| `/opt/hank-workspaces/<session-uuid>` | wananyun | 与 Trace/quant 无关话题的普通隔离目录，不进入 Git 同步 | 对应话题 Agent |
| `refs/remotes/wananyun/trace-production` | 本机 | 最近一次拉回的生产快照 | `make sync-server-agent` |
| `refs/remotes/wananyun/feishu/*` | 本机 | 最近一次拉回的话题分支快照 | `make sync-server-agent` |

运行 release 目录和 Git 分支职责不同：`/opt/hank*/current` 表示当前运行产物，
`trace-production` 表示产物对应的源码基线。只有健康检查成功后才能推进生产基线。

wananyun 的 `/opt/hank-src` 主工作树允许停留在旧 checkout；不得用它的文件内容判断生产版本。
SSH 排查时应读取 ref：

```bash
git -C /opt/hank-src rev-parse trace-production
git -C /opt/hank-src show trace-production:docs/src/operations/server-agent-sync.md
```

Trace/quant 飞书话题 worktree 从 `trace-production` 创建，因此会得到当前协议文件；普通隔离目录不加载本协议。

## 工作区路由

1. 命令、帮助、问候和尚未形成具体事项的消息路由到 conversation Agent，只创建无工作目录的 session，不创建工作区。
2. `feishu_chats` 已有映射时，历史话题始终复用原 session/workspace，不重新分类。
3. 新消息由路由 Agent 标记为 `conversation`、`trace_code`、`quant_code` 或 `general_task`，同时选择 `native`、`codex` 或 `claude`，并把 `agent_backend`、`agent_kind`、`workspace_kind` 写入 session metadata。两类代码 Agent 使用 Git worktree，通用任务使用普通隔离目录；判断失败默认 `general_task + 当前有凭据的 CLI`，避免误触生产仓库。Codex 只自动复用官方 OpenAI Responses provider，第三方 Responses 兼容端点通过 `make sync-agent-cli-config` 从本机生效配置同步。
4. 同一飞书话题始终复用首次确定的 backend、workspace 和 `agent_thread_id`。只有 `/new` 或新话题会重新路由；不得因后续消息再次创建 worktree。
5. 只有 repository workspace 可以运行 `/diff`、`/test`、`/deploy`、`/rollback`，普通 workspace 只能进行该话题范围内的通用工作。

## 数据流

```text
本机 master / GitHub
        │ 本机 bundle + SSH 手工部署
        ▼
wananyun Git 对象库 ──→ trace-production ──→ 新建 feishu/*
                              ▲                    │
                              └── 审批部署成功 ────┘
                              │
                              │ make sync-server-agent（只拉回）
                              ▼
本机 refs/remotes/wananyun/*
        │ 人工检查 + merge
        ▼
本机 master ──→ 人工 push GitHub
```

同步脚本不形成自动闭环。`merge` 和 `push` 必须保留为本机人工动作，避免服务器代码未经复核直接进入上游。

## 飞书侧标准流程

1. 在新话题描述需求，server 先固定 Agent 后端与任务类型，再按工作区路由规则创建 Git worktree、普通隔离目录或不创建目录。
2. Agent 开始工作前读取 `AGENTS.md` 和本协议；修改 quant 时还必须读取 `quant/AGENTS.md`。
3. Agent 只修改允许的项目目录，不修改 `client/`、生产配置或部署基础设施。
4. 在同一话题迭代，用 `/diff` 检查范围，用 `/test` 运行固定测试矩阵。
5. 用 `/deploy` 生成审批卡，由发起任务的管理员确认。
6. helper 从已提交 commit 构建不可变 release，切换后逐项健康检查。
7. 全部成功后 helper 将 `trace-production` 快进到该 commit；失败则回滚本次切换且不推进基线。

以下内容属于部署基础设施，只能在本机修改并走 SSH 部署，不允许飞书 Agent 自部署：

```text
deploy/
Makefile
config.example.toml
systemd / nginx / sudoers
```

## 本机拉回流程

在仓库根目录执行：

```bash
make sync-server-agent
```

脚本执行以下固定动作：

1. SSH 到 wananyun，以 `hank` 用户从所有本地分支生成临时 Git bundle。
2. 下载 bundle，仅导入 `trace-production` 和 `feishu/*` 到 `refs/remotes/wananyun/*`。
3. 检查生产差异和所有话题分支不包含 `client/`。
4. 输出两端提交关系、待拉回提交和尚未部署的话题分支。
5. 删除两端临时 bundle。

脚本不会切换当前分支、修改工作区、merge、rebase、push，也不会连接 GitHub。

### 生产分支可快进

先审阅，再人工合并：

```bash
git log --stat HEAD..refs/remotes/wananyun/trace-production
git diff HEAD..refs/remotes/wananyun/trace-production
git merge --ff-only refs/remotes/wananyun/trace-production
git push origin master
```

### 两端已经一致

不需要 merge 或 push。脚本会输出相同的两个 SHA。

### 本机领先 wananyun

通常表示本机提交尚未部署。先确认提交已经人工 push，再按变更目标执行本机部署脚本；
不要从 wananyun `fetch origin`。

### 两端分叉

同步脚本拒绝自动合并。先查看两侧独有提交：

```bash
git log --left-right --graph --oneline \
  HEAD...refs/remotes/wananyun/trace-production
```

在本机创建协调分支，人工选择 merge、rebase 或 cherry-pick。不得强推 wananyun 分支，
不得删除服务器独有 commit。协调结果完成测试并 push GitHub 后，再通过本机部署脚本建立新的生产基线。

## 本机向 wananyun 发布

本机开发的提交遵循：

```text
提交并测试 → 人工 push GitHub → make deploy* → 健康检查 → trace-production
```

`bootstrap-server-agent` 和手工部署脚本通过本机生成的 Git bundle 交付 commit；
GitHub `origin` 在 wananyun 只保留为 break-glass 元数据。禁止把 GitHub token、SSH 私钥或其他 push 凭据部署到 wananyun。

## Claude Code / Codex 配置同步

外部 Agent 的凭据有两条来源，`cli_agent` 每轮任务按以下优先级解析：

1. **admin「Agent CLI」页**（`agent_cli_configs` 表）——启用且填了凭据的行优先。改完下一轮任务即生效，不重启服务。
2. **`/opt/hank/agent-cli.env`**——库里没有行、行被停用或没填凭据时兜底。登服务器改文件的应急路径保持可用，改完仍需重启 `hank-server`。
3. **server 中已启用的 provider 记录**——两者都没有时的最后回退。

日常轮换第三方 API 端点走 admin 页即可：本机 `pnpm dev` 起的 admin 默认代理到线上 server（见 `admin/vite.config.ts`），所以本地打开页面改的就是部署环境的配置。dev 自身的 `server_agent.enabled=false`，不会真的拉起外部 CLI。

页面上「当前生效」一行显示实际来源，用来区分「存了配置但没启用，其实还在用环境文件」。「测试连通性」按钮用配置里的凭据向端点发一次最小推理请求；Codex 走 Responses API，与真实调用同协议，能暴露「Chat Completions 通但 Responses 不支持」的中转。

凭据只在服务端注入子进程环境，GET 接口从不回传明文，只回传是否已设置。admin 可配置的附加环境变量限定在模型与输出上限白名单内，其他键一律拒绝。改端点或模型时凭据留空即保留原值。

注意「停用」与「清除凭据」不同：停用只是不再使用这一行、回退到环境文件，凭据仍存在库里；轮换掉泄露的 key 要用「清除凭据」真正删除该行。

### 批量复用本机配置

wananyun 没有 GUI，不安装 CC Switch。需要一次性把本机已经生效的完整配置搬过去时，在仓库根目录执行：

```bash
make sync-agent-cli-config
```

脚本只同步 `~/.claude/settings.json`、`~/.codex/auth.json` 和 `~/.codex/config.toml`，不传历史会话、缓存或插件。完整源文件安装到 `/home/hank` 的标准配置目录，并在 `/opt/hank-agent-config/current` 保存一份 `root:hank` 受限副本；飞书 Agent 只使用结构化提取到 `/opt/hank/agent-cli.env` 的认证、端点和模型白名单。Codex 当前 Provider 必须使用 Responses API。

注意该脚本写的是上面第 2 层的环境文件。如果 admin 里已有启用的配置行，它的优先级更高，同步后不会生效——需要在 admin 停用对应后端，或直接在 admin 里改。

配置文件和本地/远端临时目录均不得进入 Git。同步脚本不输出凭据，远端文件权限必须保持 `root:hank 0640` 或 `hank:hank 0600`；脚本完成后重启 `hank-server`，使 systemd 重新加载环境文件。

## 强制边界

- `client/`：不得由飞书 Agent 读取、修改、测试或部署；同步发现相关变更必须失败。
- `config.toml`：不得提交；线上配置只通过 SSH 维护。
- `quant/alembic/`：常规部署拒绝执行 migration。维护窗口中通过 SSH 备份、迁移、验证。
- GitHub：wananyun 正常流程不访问；只有本机人工 push。
- 历史：同步只创建或更新本机远端跟踪引用，不自动改写任何本地分支。
- 部署：只有审批 commit 与当前 `trace-production` 基线一致时才能执行。

## 协议维护

双方共同维护本文件，但同一时刻只以 `trace-production` 上的版本作为飞书侧规则，以本机 `master` 上的版本作为本机侧规则。
`make sync-server-agent` 将两者带回同一 Git 历史，人工 merge 后完成统一。

- 仅补充说明或故障案例：可从本机或飞书话题修改本文件，按各自标准流程提交。
- 改变 ref 映射、同步命令、权限边界或安全行为：必须在本机同时修改本文件、相关脚本和测试；飞书 Agent 应停止并请求本机维护。
- 不在 `AGENTS.md`、`docs/feishu.md` 或其他文档复制完整步骤，只保留指向本文件的入口。
- 修改协议后递增顶部版本，并在 commit message 中使用 `docs(deploy):` 或相应的 `feat/fix(deploy):`。

## 每次同步检查表

- [ ] 当前目录是 Trace 仓库，工作树状态已知。
- [ ] 已运行 `make sync-server-agent`，且记录两端 SHA 和分支关系。
- [ ] 拉回差异不包含 `client/`、配置密钥或未知大文件。
- [ ] 已检查尚未部署的 `wananyun/feishu/*` 分支。
- [ ] 只在可快进且审阅通过时使用 `git merge --ff-only`。
- [ ] GitHub push 由本机人工执行。
- [ ] 部署后 server/CLI/quant/静态站按实际目标完成健康检查。
- [ ] 再次运行同步，确认 `trace-production` 与预期 commit 一致。
