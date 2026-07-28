# 书库在框架中的位置：假说源，不是策略库

## 1. 角色分工

| 来源 | 产出 | 不产出 |
|---|---|---|
| 书籍 / 笔记 | 假说、失败模式、否决灵感、候选 ID | 已验证 Alpha、默认参数真理 |
| StrategySpec | 可编程完整策略 | 名人背书 |
| 回测 + 实验 | 本系统证据 | 交易指令 |

```text
docs/research/notes/*.md
        │  提炼为假说 + 可客观化规则草案
        ▼
前端配置 StrategySpec（或实验冻结）
        │
        ▼
本系统回测 / OOS / 多重检验
        │
        ▼
evidence_status + 归档
```

## 2. 从书到 Spec 的最小映射

| 书中内容类型 | 框架落点 |
|---|---|
| 进场规则 | `entry.condition` |
| 离场规则 | `native_exit` |
| 止损 % | `overlays.risk`（覆盖层，默认可关） |
| 市场状态过滤 | entry 的 `all` 子条件或 risk_filter |
| 组合选股 | portfolio `score` + `top_n` |
| 「不可程序化」 | 不进 Spec；人工研究字段 |
| 治理原则 | 实验流程 / validation，不是信号 |

`metadata.sources` 可写 `book` + `candidate_id`，**仅追溯**，不提高证据等级。

## 3. 书库候选如何进入队列

`docs/research/strategy-candidates.md` 中的 canonical 候选：

1. 看「当前系统可验证性」  
2. `supported` 类 → 直接配 Spec 进批次 1  
3. `missing_data` → 先数据，不硬编代理冒充  
4. `boundary_denied` → 永不配置为可回测策略  

**不按书名气排序，按可配置性与消融成本排序。**

## 4. 反向价值如何进入框架

书中失败经验 → `validation.rejection_rules` / 实验淘汰条件 / UI 文案：

| 失败模式 | 框架表达 |
|---|---|
| 过拟合参数 | parameter_scans + unstable_parameters |
| 无原生离场 | Spec 校验拒绝 |
| 涨停神回测 | 撮合层不可成交 |
| 幸存者 | 动态池 + 退市样本 |
| 只看胜率 | 报告多指标，否决可盯 OOS 年化/回撤 |
| 秘术归因 | sources 标不可信；仍须独立 Spec |

## 5. 禁止事项

- 禁止「一键导入本书全部策略」类功能（变相未验证上架）  
- 禁止系统策略以书名命名暗示已验证（系统种子保持中性描述）  
- 禁止把笔记中的固定数字写成全局默认  

## 6. 正确示范

> 「趋势技术分析提出：收盘突破 + 相对放量可能有延续。」  
> → 配 Spec A；去掉放量得 Spec B；同区间实验。  
> → 无论结果如何，归档。书仍只是假说来源。
