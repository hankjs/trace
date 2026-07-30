# quant 系统介绍（Slidev）

面向**金融小白**的 quant 系统讲解稿：产品边界、术语（指标 / 因子 / 策略 / 回测指标）、业务架构与日常研究流程。配**线上界面实拍**，图文对照。

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
```

界面截图在 `public/screenshots/`（构建时打进静态站）。

## 内容结构

| 章节 | 内容 |
|---|---|
| 一 | 定位与登录页边界 |
| 二 | **界面导览**（总览 / 选股 / 信号 / 策略 / 回测 / 词典…） |
| 三 | 术语：指标、因子、估值、回测指标 |
| 四 | 策略 Spec、预置、信号含义 |
| 五 | 业务分层、流水线、证据状态机 |
| 六 | 一日研究闭环与误区 |
| 附录 | 速查与命令 |

主文件：`slides.md`。口径以仓库 `README.md`、`PRODUCT.md`、`DATA-ARCHITECTURE.md`、`app/catalog.py` 为准。
