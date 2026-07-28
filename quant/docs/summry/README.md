# Trace Quant 策略验证框架

> **目标不是把书里的策略搬进系统。**  
> **目标是：任何可客观化的日频策略，都能在前端配置 → 校验 → 回测 → 实验归档。**  
> 书籍只提供假说与反例；有效性只认本系统证据。

## 文档

| 文件 | 内容 |
|---|---|
| [00-framework-overview.md](./00-framework-overview.md) | 架构总览：数据流、分层、产品边界 |
| [01-lifecycle.md](./01-lifecycle.md) | 从假说到证据的完整生命周期 |
| [02-how-to-add-strategy.md](./02-how-to-add-strategy.md) | 如何新增策略（配置优先，扩组件次之） |
| [03-extension-points.md](./03-extension-points.md) | 扩展点：操作符 / 字段 / 基线 / 否决规则 |
| [04-frontend-map.md](./04-frontend-map.md) | 前端页面与 API 对照 |
| [05-book-as-hypothesis.md](./05-book-as-hypothesis.md) | 书库如何喂入框架（假说，非模板） |
| [06-gap-and-roadmap.md](./06-gap-and-roadmap.md) | 当前能力缺口与优先补强 |
| [REQ-framework-capabilities.md](./REQ-framework-capabilities.md) | **需求**：片段库 / 实验对比 / 高复用算子 / design_complete 硬清单 |

## 一句话

```text
假说 → StrategySpec(JSON) → 能力解析 → 编译目标仓位 → T+1 回测 → 实验/证据状态
         ↑ 前端配置              ↑ 无任意代码
```

实现主路径已在代码中：

- 规格：`app/strategy/spec.py`
- 组件：`app/strategy/components.py`
- 编译：`app/strategy/compiler.py`
- 回测：`app/backtest/` + `validation.py`
- 实验：`app/experiment/`
- 前端：`web/src/views/Strategies.vue`、`Experiments.vue`、`Backtest.vue`
