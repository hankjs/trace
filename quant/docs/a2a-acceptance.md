# quant × Trace A2A 验收记录

| 属性 | 内容 |
|------|------|
| 关联设计 | `quant/docs/a2a-design.md`（DESIGN-A2A-2026-07） |
| 验收日期 | 2026-08-01 |
| 验收方式 | 自动化测试（quant pytest 802；Rust hank-a2a-client 14 + code-tools 36 + hank-server 85，另 2 ignored；hank-server build；web pnpm build）+ 本地真实 LLM 端到端（Trace + quant + MySQL，SSE 驱动确认恢复） |

## 交付落点（回填 §18）

| 项 | 实际路径 |
|----|----------|
| quant A2A 包 | `quant/app/a2a/`（card/auth/server/tasks/gaps + skills/ 19 个 handler） |
| 因子评估引擎 | `quant/app/factors/evaluation.py` + `quant_factor_evaluation` 表 + REST `GET /api/factors/evaluations[/{id}]` |
| 新表/迁移 | `quant_a2a_audit` / `quant_research_finding` / `quant_factor_evaluation`（alembic `0025_a2a_tables`） |
| Trace Client | `crates/hank-a2a-client`（JSON-RPC + SSE + resubscribe + list） |
| quant_* 工具/闸门 | `crates/code-tools/src/quant_tools.rs` / `quant_grant.rs`；拦截层 `crates/code-agent/src/session.rs`；恢复解析 `server/src/chat.rs` |
| Orchestrator skill | `server/skills/quant-research/SKILL.md`（注册 quant 工具的会话全量注入系统提示 + SkillInfo 索引） |
| 微信 | 代签复用 `weixin/router.rs sign_internal_jwt`（内部函数，无 HTTP 端点）；待确认单进程内 map + 5min TTL |
| 缺口消费 | `system.gap_summary` + REST `GET /api/admin/a2a-gaps`（同源 `app/a2a/gaps.py`）+ web 页 `quant/web/src/views/AdminGaps.vue` |

## §15.1 协议与安全

| # | 结果 | 证据 |
|---|------|------|
| 1 | ✅ | 无/坏 JWT → Task failed「未登录」；test_missing/bad_jwt + 冒烟 |
| 2 | ✅ | 冒烟：backtest.run 完成 run_id=22，REST `GET /api/backtest/22` 200；evidence 推进走 `advance_after_backtest`（unverified 起点按既有闸门不自动升级，与 REST 完全一致） |
| 3 | ✅ | `_prepare_backtest` capability 硬闸；validate 非 supported 不进执行 |
| 4 | ✅ | `tests/test_backtest_cancel.py`：pending 即取消；running 检查点协作中断，无半写 metrics |
| 5 | ✅（代码层） | skill 话术约束 + 工具摘要不产交易措辞；最终措辞依赖 LLM 遵守，建议首次微信联调时人工抽查 |
| 6 | ✅ | 仅 text → failed 并列出全部 skill id；冒烟 + test_text_only |
| 7 | ✅ | 冒烟 Card：streaming=true, pushNotifications=false, 19 skills |
| 8 | ✅ | quant pytest 802 全绿（含既有回归）；web pnpm build 通过 |
| 9 | ✅ | 互斥 409 → Task failed（可等待/可先 Cancel 文案）；复用 quant_task 单任务互斥 |
| 10 | ✅ | Rust 侧模型自填 confirmed 无条件剥离（quant_tools）；quant 二次校验（冒烟：无 confirmed → failed）；微信白名单单测 |
| 11 | ✅ | 缺 strategy_id / initial_cash / fees / slippage_bps / params / 裸 spec → 可操作文案；冒烟 + 3 个测试 |
| 12 | ✅ | alert_level=ok（冒烟实测）；validate 用 valid/spec_hash |
| 13 | ✅ | 冒烟：enabled=false + research_status=unverified + parent_strategy_id 键存在；免确认 |
| 14 | ✅ | 冒烟：trial 与 backtest.run 同 client_request_id 重发返回同一 trial/run，审计不重复计次；test_high_cost_idempotent_replay |
| 15 | ✅ | 冒烟：artifact `backtest_list`（修复后）items/has_more；仅本人；字段与 get 同源 `_run_summary` |
| 16 | ✅ | factor validate/preview REST 放宽 require_client；preview 7 codes → 5 + truncated_codes（冒烟）；save_draft 非 admin 拒 + enabled:true 拒（测试） |
| 17 | ✅ | validate 失败写 failure_kind/missing_capability；冒烟 + test_validate_failure_writes_audit_gap_columns |
| 27 | ✅ | sign_internal_jwt 仅 router.rs 内部调用，路由表无暴露（W6 审查） |
| 28 | ✅ | tasks/list 已挂载（冒烟返回本人长任务列表）；resubscribe 由 SDK 队列管理器提供 |

## §15.2 Experiment 主链

| # | 结果 | 证据 |
|---|------|------|
| 18 | ✅ | 缺 strategy_id → failed（冒烟）；有 id 冻结 spec（frozen_spec_hash 与策略 spec_hash 一致） |
| 19 | ✅ | 冒烟：trial 调 create_trial_and_run，outcome=rejected 也落 trial；evidence 未自动改；artifact 含 promotion 只读摘要 |
| 20 | ✅ | 9 patch → failed「一次最多 8 个」；配额按条数（quota_tracker cost=len(patches)） |
| 21 | ✅ | experiment.get 返回 trials/multiplicity/pending_promotions（冒烟 + test_experiment_get_returns_registry_fields）；无 accept skill |

## §15.3 研究 Agent（Trace）

| # | 结果 | 证据 |
|---|------|------|
| 22 | ✅（真实 LLM） | 会话 `27d7040b-049d-49e6-b8f3-650ea62e67b6`：一句话意图后 `catalog → data_quality → validate → save_draft → create_experiment → 系统确认 → trial → get → report_finding → Conclude`；strategy_id=20、experiment_id=3、trial_id=4、run_id=23 |
| 23 | ✅（真实 LLM） | 同会话默认走路径 E，全程未调用 `quant_run_backtest`；单次 trial 只表述为样本内结果与证据不足，不称科学验证通过 |
| 24 | ⚠️ 待真实 LLM | S3/S5 停止条件与「停止后禁高成本」已写入 skill；尚缺分别触发 S3、S5 的真实会话证据 |
| 25 | ✅（真实 LLM） | 会话 `f5dfe829-334d-4bb2-95dd-decd99c92102`：分钟 K 线 + 实时盘口请求仅调用 `quant_catalog → quant_report_finding`，0 次确认、0 个高成本工具；S6 `product_gap` inserted=1，补强建议保持日频边界并改用外部专用系统 |
| 29 | ⚠️ 待真实 LLM | 因子 validate/preview/evaluate/save_draft 的确定性链路与测试已就绪；尚缺一次带真实确认、样本期 / n_periods / 多重检验措辞的真实 LLM 会话 |
| 30 | ✅（真实 LLM） | S6 会话的 `quant_report_finding` payload 与最终展示逐字段一致并落表 inserted=1；路径 E 也实际调用了 report_finding。新增规则禁止把「证据不足但未拒绝」伪记为 `hypothesis_rejected`，此时必须上报空数组 |

## §15.4 缺口闭环

| # | 结果 | 证据 |
|---|------|------|
| 26 | ✅ | 冒烟：gap_summary merged 排行 + admin REST `GET /api/admin/a2a-gaps` 同源 |
| 31 | ✅ | 运行期失败写缺口列（冒烟：runtime_error 审计行；trial outcome=error 提取逻辑于 server._audit_info） |
| 32 | ✅ | 双源分列 + 合并排行（冒烟：audit 2 + finding 1 → merged 3）；test_gap_summary_scope_me_no_cross_table_leak |

## 实现期发现并修复的问题

1. `gap_summary` scope=me 时 findings 查询误用 `A2aAudit.user_id` 过滤（跨表笛卡尔积）→ 拆为 audit_filter/finding_filter + 回归测试。
2. `backtest.list` artifact 误名 `items` → 对齐契约 `backtest_list` + 回归测试。
3. `backtest.run` 内部 `_BacktestIn` 缺 `params` 属性（`_prepare_backtest` 依赖）→ 补齐并注释契约禁止 payload 携带。
4. `backtest.run` 幂等重放空 artifact（backtest 任务类型不写 task.result）→ 完成后补写 `{"backtest_summary": ...}`。
5. `_run_summary.validation` 未平铺 verdict/reasons → 对齐 §8.4 形态（保留 baselines/oos/rejection 全量）。
6. 审计 run_id/experiment_id/trial_id 恒 NULL → `_audit_refs` 从 artifact 提取。
7. trial outcome=error 未写缺口列 → `_audit_info` 增加运行期失败提取。
8. trial/trial_batch/factor.evaluate 等待循环忙轮询（无 sleep）→ 统一 `_common.wait_for_task`（0.5s 间隔 + 协作取消等待状态翻转）。
9. Trace 侧多个 agent 的 `cargo fmt --all` 噪音 → 已全部还原，diff 仅保留实质改动。
10. Trace A2A client 把配置的服务根地址 POST 到 `/`，quant RPC 实际为 `/a2a` → client 统一规范化根地址与完整 endpoint，并以真实 uvicorn 日志确认请求全部命中 `POST /a2a`。
11. `quant_catalog` 工具提示模型请求不存在的 `snippets/operators`，backend 又静默忽略未知 section → 新增运行时真源生成的 `strategy_authoring`（字段、精确算子形状、限制、完整合法示例），section 改为枚举并对未知值返回可纠错错误。
12. ask_user 发出 TurnComplete 后、消息链持久化前客户端可立即回复，导致确认 tool_result 脱离历史 → chat SSE 将 TurnComplete 延迟到 Agent 返回且消息保存完成后广播；真实会话确认后成功继续 trial。
13. 真实 trial 暴露「证据不足但未拒绝」被误记 `hypothesis_rejected` → skill + tool schema 明确仅工具给出 rejected/verdict 拒绝时可用该 kind；无缺口的证据不足结论调用 report_finding 传空数组。

## 冒烟残留（开发库）

本地 MySQL 中保留此前协议冒烟产物，以及本轮真实 LLM 验收记录：strategy_id=19/20、experiment_id=2/3、trial_id=3/4、run_id=23、对应 audit/findings 与 Trace 会话。strategy 20 / experiment 3 / trial 4 / run 23 是成功回测；trial 3 是数据不足错误；路径 E 首次结论还留有一条语义错误的 `hypothesis_rejected` finding，规则已修复但未擅自改写历史记录。以上均未自动删除。
