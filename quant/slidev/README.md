# quant 系统介绍（Slidev）

面向**金融小白**的 quant 系统讲解稿：产品边界、术语（指标 / 因子 / 策略 / 回测指标）、业务架构与日常研究流程。

## 启动

```bash
cd quant/slidev
pnpm install
pnpm dev          # 本地预览，默认 http://localhost:3030
```

其他命令：

```bash
pnpm build        # 导出静态站点到 dist/
pnpm export       # 导出 PDF（需额外浏览器依赖，见 Slidev 文档）
pnpm preview      # 允许局域网访问

# 仓库根目录
make quant-slidev          # 同上 pnpm dev
make quant-slidev-build    # 同上 pnpm build
make deploy-quant-slidev   # 部署到 wananyun (nginx :3030 → /opt/hank-quant-slidev)
# SSH_HOST=其他主机 make deploy-quant-slidev
```

## 内容结构

| 章节 | 内容 |
|---|---|
| 一 | 定位与产品边界（研究 vs 交易） |
| 二 | 零基础术语：日线、指标、因子、基本面、回测指标 |
| 三 | 策略 Spec、六套预置、信号含义 |
| 四 | 业务分层、数据生命周期、选股与晚间流水线、证据状态机 |
| 五 | 前端工作区与一日研究闭环 |
| 六 | 误区与原则 |
| 附录 | 速查卡、代码目录、启动命令 |

主文件：`slides.md`。细节与口径以仓库 `README.md`、`PRODUCT.md`、`DATA-ARCHITECTURE.md`、`app/catalog.py` 为准。
