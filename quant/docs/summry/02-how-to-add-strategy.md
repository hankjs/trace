# 如何新增一条可验证策略

## 决策树

```text
新想法
  │
  ├─ 能用现有操作符 + 字段表达？
  │     └─ YES → 前端配置 Spec，保存，回测  ✅ 主路径
  │
  ├─ 需要新「通用」运算（如 rolling_std）？
  │     └─ YES → 扩 components + spec 白名单 + 测试 + 前端编辑器
  │              再配置 Spec
  │
  ├─ 需要新数据字段（如单季扣非 EPS）？
  │     └─ YES → 数据管道 + SUPPORTED_FIELDS + 点时可用性
  │              再配置 Spec
  │
  ├─ 无法客观化（手绘形态、盘感、护城河故事）？
  │     └─ 仅人工研究字段 / subjective_only，不进回测
  │
  └─ 越界（下单、期权执行、高频）？
        └─ boundary_denied，不进入开发
```

**禁止默认路径：为每个策略新建 `strategies/xxx.py`。**

---

## 主路径：纯配置（推荐）

### 1. 写清假说（进 Spec.metadata）

- `hypothesis`：机制一句话  
- `canonical_id`：研究编号（如 `CAN-TRD-02-v1`）  
- `sources`：可选，书名 + 候选 ID（追溯用，不增加证据权重）  
- `evidence_status`：系统管理，勿手改跳级  

### 2. 定义 universe

- `pool_id`、是否排除 ST、最小上市天数、日均成交额门槛  

### 3. 声明 data_requirements

表达式用到的每个字段必须声明，且 `required=true`。  
前端「补全缺失字段」可辅助，以服务端校验为准。

### 4. 写 entry / native_exit（single）

示例：收盘突破过去 20 日最高价（不含当日），量比 > 1.5；  
跌破过去 10 日最低价离场。

```json
{
  "entry": {
    "condition": {
      "op": "all",
      "args": [
        {
          "op": "gt",
          "left": {"op": "field", "name": "close"},
          "right": {
            "op": "rolling_max",
            "input": {"op": "field", "name": "high"},
            "window": 20,
            "shift": 1
          }
        },
        {
          "op": "gt",
          "left": {
            "op": "volume_ratio",
            "input": {"op": "field", "name": "volume"},
            "window": 20,
            "shift": 1
          },
          "right": {"op": "literal", "value": 1.5}
        }
      ]
    },
    "reason_code": "breakout_entry"
  },
  "native_exit": {
    "condition": {
      "op": "lt",
      "left": {"op": "field", "name": "close"},
      "right": {
        "op": "rolling_min",
        "input": {"op": "field", "name": "low"},
        "window": 10,
        "shift": 1
      }
    },
    "reason_code": "channel_exit"
  },
  "positioning": {"type": "binary", "target": 1.0}
}
```

### 5. 可选 overlays

- 风险：固定 % 或 ATR 倍数（默认研究时常关闭，做消融时再开）  
- 止盈：同上  
- 覆盖离场 ≠ 原生离场  

### 6. 配置 validation

- 基线、`locked_oos`、否决规则、参数扫描路径  

### 7. 校验 → 保存 → 回测 →（可选）实验

API：

- `POST /api/strategies/validate`  
- `POST /api/strategies` / `PATCH /api/strategies/{id}`  
- 回测创建接口（见 Backtest 页）  
- `POST /api/experiments` + trials  

---

## 消融模板（框架内置用法）

| 版本 | 改动 | 目的 |
|---|---|---|
| A | 完整 Spec | 主候选 |
| B | 去掉量比 | 量条件是否有增量 |
| C | 去掉市场无关过滤 | … |
| D | 仅开 ATR 覆盖层 | 覆盖层成本 |
| E | 换 exit 窗口 | 参数敏感 |

全部同一池、同一区间、同一费用；结果进实验或并列回测记录。

---

## 扩组件路径（次选）

当且仅当多个未来策略都会用到同一语义时：

1. `spec.py`：`SUPPORTED_OPERATORS` + `_OP_FIELDS` + 类型检查  
2. `components.py`：确定性实现 + `COMPONENT_VERSIONS`  
3. 单元测试：边界、NaN、无前视（shift）  
4. 前端 `SpecExpressionEditor` / 操作符目录  
5. 更新 catalog 研究词典  
6. **不**写具体策略 Python 文件  

---

## 扩字段路径

1. 入库与 `available_date`  
2. `SUPPORTED_FIELDS`  
3. 编译时 fields 注入（engine/runtime）  
4. 能力解析能区分 missing_data  

---

## 检查清单（上线前）

- [ ] 进场与原生离场齐全（或组合权重完整）  
- [ ] `capability.status == supported`  
- [ ] 无未来函数（rolling `shift>=1` 或 cross 语义正确）  
- [ ] 声明了全部 required 字段  
- [ ] 写了假说与至少一条否决/基线  
- [ ] 未连接交易、未输出真实仓位指令  
