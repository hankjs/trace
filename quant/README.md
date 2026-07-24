# quant — A股日频量化信息系统

纯信息系统:只做行情、指标、信号、记账与回测,**不做任何自动交易**。
与主仓库 server 零代码耦合,仅共用同一个 MySQL 数据库,表名全部 `quant_` 前缀。

## 环境准备

```bash
cd quant
uv sync          # 自动创建 venv 并安装依赖(requires-python: >=3.11,<3.14)
```

数据库连接默认读取仓库根目录 `config.toml` 的 `[server].database_url`
(`mysql://` 自动转为 `mysql+pymysql://`)。如需覆盖,编辑本目录的 `config.toml` 的 `[quant]` 段。

## 启动

```bash
uv run uvicorn app.main:app --port 8100
# 开发热重载: uv run uvicorn app.main:app --port 8100 --reload
```

启动时自动 `create_all` 建表(`quant_stock / quant_daily_bar / quant_snapshot /
quant_signal / quant_trade / quant_backtest_run / quant_backtest_equity`),
并启动 APScheduler:

- 交易日(周一~周五)16:30:baostock 盘后日线增量 + akshare 对账 + 策略信号计算
- 交易日盘中 9:30–15:00 每 30 分钟:akshare 快照落 `quant_snapshot`

法定节假日不做专门日历判断:盘后任务会因当日无数据而自然空跑。

## 数据回填

```bash
# 回填单只股票(默认从 config 的 backfill_start 起到今天)
curl -X POST "http://localhost:8100/api/admin/backfill?code=sh.600519&start=2024-01-01"

# 导入全市场股票列表(可选)
curl -X POST "http://localhost:8100/api/admin/import-stocks"

# 加入自选 + 手动跑一次信号
curl -X POST "http://localhost:8100/api/watchlist" \
  -H "Content-Type: application/json" -d '{"code":"sh.600519","name":"贵州茅台"}'
curl -X POST "http://localhost:8100/api/admin/run-signals"
```

数据口径:baostock 前复权价(`adjustflag=2`)落 `open/high/low/close`,
不复权收盘价落 `raw_close`;策略、回测、信号全部跑在前复权序列上。

数据源降级:akshare 东财接口(`stock_zh_a_spot_em` / `stock_zh_a_hist`)
在部分网络下会被断连,快照与日线对账会自动降级为新浪源
(`stock_zh_a_spot` / `stock_zh_a_daily`,较慢,快照约 20~30 秒)。

## API 一览

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | 健康检查 |
| GET | `/api/market/kline?code=&start=&end=` | K线(前复权) |
| GET | `/api/market/snapshot` | 自选股最新价(快照优先,否则最近收盘) |
| GET/POST/DELETE | `/api/watchlist`、`/api/watchlist/{code}` | 自选股管理 |
| GET | `/api/signals?date=&code=&strategy=&side=` | 信号查询 |
| GET/POST/DELETE | `/api/portfolio/trades`、`/api/portfolio/trades/{id}` | 手工成交记账 |
| GET | `/api/portfolio/positions` | 持仓(均价法成本、浮动盈亏) |
| GET | `/api/backtest/strategies` | 可选策略列表 |
| POST | `/api/backtest` | 发起回测(同步执行并落库) |
| GET | `/api/backtest/{id}` | 回测结果(含净值曲线) |
| POST | `/api/admin/backfill?code=&start=` | 手动历史回填 |
| POST | `/api/admin/run-signals?date=` | 手动信号计算 |
| POST | `/api/admin/snapshot` | 手动抓一次快照 |
| POST | `/api/admin/import-stocks` | 导入股票列表 |

交互式文档:`http://localhost:8100/docs`

## 内置策略

- `ma_cross`:双均线金叉/死叉(默认 MA5/MA20,`params: {"fast":5,"slow":20}`)
- `breakout`:N 日突破(默认 20 日新高入场、10 日新低出场,`params: {"entry":20,"exit":10}`)

回测规则:信号次日开盘价成交;费用默认佣金万 2.5 双边 + 卖出印花税 0.05% +
滑点万 1(可在请求 `costs` 中覆盖);多标的资金等分。

## 目录结构

```
quant/
├── pyproject.toml / config.toml / README.md
└── app/
    ├── main.py            # FastAPI 入口
    ├── config.py          # 读根 config.toml + 本目录覆盖
    ├── db.py / models.py  # SQLAlchemy(quant_* 表)
    ├── scheduler.py       # APScheduler 定时任务
    ├── data/              # baostock / akshare 客户端 + 入库/对账
    ├── indicators/        # MA/EMA/MACD/RSI/ATR/量比(纯 pandas)
    ├── strategy/          # 策略引擎 + ma_cross / breakout
    ├── portfolio/         # 记账 + 持仓推导
    ├── backtest/          # 日频向量化回测
    └── api/               # FastAPI 路由
```
