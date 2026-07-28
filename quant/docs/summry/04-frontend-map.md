# 前端与 API 地图

## 1. 页面职责

| 页面 | 路由用途 | 框架角色 |
|---|---|---|
| **策略** `Strategies.vue` | 创建/编辑/复制/校验 Spec | 配置入口 |
| **回测** `Backtest.vue` | 选策略跑区间、看 validation | 单次证据 |
| **实验** `Experiments.vue` | 冻结规格 + 参数 trial | 族级证据 |
| **研究计划** | 日频信号解释与计划版本 | 运行时消费 Spec |
| **研究词典** catalog | 字段/操作符/指标中文名 | 配置辅助 |
| **持仓/交易** | 手工记账 | **不**执行策略信号 |

## 2. 策略页关键交互

| UI | 后端 |
|---|---|
| Spec 表单 `StrategySpecEditor` | 构建 JSON Spec |
| 表达式 `SpecExpressionEditor` | AST 节点编辑 |
| 校验 | `POST /api/strategies/validate` |
| 保存/更新 | `POST/PATCH /api/strategies` |
| 能力徽章 | `capability` 字段 |
| 证据操作 | 标记 design_complete / 否决等 |
| 复制 | `duplicate`：快速做消融副本 |

系统策略：只读，可复制后改。

## 3. 回测页关键交互

| UI | 含义 |
|---|---|
| 选策略 + 区间 + 成本 | 一次 run |
| 净值/回撤/交易明细 | 撮合结果 |
| validation.baselines | 与基线对比 |
| validation.oos / verdict | 锁定样本外与否决 |
| 退出原因分布 | 原生 vs 覆盖 vs 强制 |

结果绑定：`strategy_spec_snapshot` + hash，可复盘「当时规则」。

## 4. 实验页关键交互

| UI | 含义 |
|---|---|
| 创建 experiment | 冻结 hypothesis + Spec |
| 添加 trial | param_patch 网格一点 |
| outcome | ok / no_trades / error / rejected |
| 归档 | 失败也保留 |

## 5. 表单 ↔ Spec 映射

`strategySpecForm.ts`：

- 表单状态覆盖 Spec 全部受控字段  
- 表达式以 AST 原样持有，不拆成「神秘参数」  
- 构建失败时仍交给后端硬校验  

**原则：前端是编辑器，真相在服务端 Pydantic。**

## 6. 扩展前端时的规则

1. 新操作符 → 更新 `specExpression` 允许列表与编辑器 UI  
2. 新 Spec 顶层字段 → 同步 `api.ts` 类型 + form 双向映射 + 编辑器区块  
3. 不要在前端实现第二套回测逻辑  
4. 能力不足时展示 `issues[].path/message`，引导扩数据或扩引擎  

## 7. 建议的 UX 增强（未实现则记 roadmap）

| 项 | 价值 |
|---|---|
| 一键「从模板复制为消融」向导 | 降低验证成本 |
| 操作符面板（拖拽/点选）代替纯树编辑 | 降低配置门槛 |
| 实验对比表（多 trial 并排） | 参数平台可见 |
| 能力缺口 → 链到字段/算子文档 | 关闭配置死胡同 |
| 验证设计检查清单勾选 | 推高 design_complete 质量 |
