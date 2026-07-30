"""配置加载:读取仓库根目录 config.toml 的 [server].database_url,
并可用 quant/config.toml 的 [quant] 段做覆盖。

环境区分:
- env=dev(默认):只跑业务 API,不启动 APScheduler(日线/盘中/估值等同步仅生产跑)
- env=prod:允许调度;仍受 scheduler_enabled 与 MySQL 互斥锁约束
优先级:环境变量 QUANT_ENV > quant/config.toml [quant].env > 默认 dev
"""
from __future__ import annotations

import os
import tomllib
from pathlib import Path

QUANT_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = QUANT_DIR.parent

# 规范化后的合法取值
ENV_DEV = "dev"
ENV_PROD = "prod"
_PROD_ALIASES = frozenset({"prod", "production"})
_DEV_ALIASES = frozenset({"dev", "development", "local"})


def normalize_env(raw: str | None) -> str:
    """将配置/环境变量规范化为 dev|prod;未知值按 dev 处理(安全默认)。"""
    if raw is None:
        return ENV_DEV
    value = str(raw).strip().lower()
    if not value:
        return ENV_DEV
    if value in _PROD_ALIASES:
        return ENV_PROD
    if value in _DEV_ALIASES:
        return ENV_DEV
    return ENV_DEV


class Settings:
    def __init__(self) -> None:
        # 根 config.toml 可选:本地开发从 [server].database_url 读库,
        # 线上部署只有 quant/config.toml 时不要求根配置存在。
        database_url = ""
        jwt_secret = ""
        root_cfg = REPO_ROOT / "config.toml"
        if root_cfg.exists():
            with open(root_cfg, "rb") as f:
                root = tomllib.load(f)
            database_url = root.get("server", {}).get("database_url", "")
            jwt_secret = root.get("server", {}).get("jwt_secret", "")
        if database_url.startswith("mysql://"):
            database_url = "mysql+pymysql://" + database_url[len("mysql://"):]

        # 默认值
        self.database_url: str = database_url
        self.jwt_secret: str = jwt_secret
        self.cors_origins: list[str] = [
            "http://localhost:5173",
            "http://localhost:3000",
            "http://127.0.0.1:5173",
        ]
        self.snapshot_retention_days: int = 30
        self.backfill_start: str = "2015-01-01"
        # 运行环境:dev 不启调度;prod 才允许定时采集/研究流水线
        self.env: str = ENV_DEV
        # 调度器跨进程互斥:多 worker/多副本部署时,即使本开关为 True,
        # 也只有抢到 MySQL GET_LOCK 的那个实例真正运行定时任务(见 scheduler.py)。
        # 置 False 可让某些实例彻底不参与调度(如纯 API worker)。
        # 注意:env!=prod 时即使本开关为 True 也不会启动调度。
        self.scheduler_enabled: bool = True
        # schema 版本不一致时是否拒绝启动(开发默认 True;临时排障可在 quant 配置关)
        self.schema_strict: bool = True
        # HTTP 入口是否异步回测(BackgroundTasks)。生产默认 True;pytest 在 conftest 关。
        self.backtest_async: bool = True
        # 全局异步任务系统(app/tasks.py):提交即返回 202、后台线程池执行。
        # 生产默认 True;pytest 在 conftest 关掉后任务提交即同步内联执行。
        self.task_async: bool = True
        # 任务执行线程池大小(每用户同时只允许一个任务,这里限制全局并发)
        self.task_workers: int = 2
        # 盘后日 K 是否走 baostock 按日批量链路(P2)。默认关闭:
        # 换算口径待 P0 spike 验证(docs/baostock-bulk-ingest.md §3),验证前不开生产。
        self.bulk_daily_bars: bool = False

        # quant 自己的覆盖配置(可选)
        cfg_env: str | None = None
        local_cfg = QUANT_DIR / "config.toml"
        if local_cfg.exists():
            with open(local_cfg, "rb") as f:
                local = tomllib.load(f).get("quant", {})
            if local.get("database_url"):
                url = local["database_url"]
                if url.startswith("mysql://"):
                    url = "mysql+pymysql://" + url[len("mysql://"):]
                self.database_url = url
            if local.get("cors_origins"):
                self.cors_origins = list(local["cors_origins"])
            if local.get("snapshot_retention_days"):
                self.snapshot_retention_days = int(local["snapshot_retention_days"])
            if local.get("backfill_start"):
                self.backfill_start = str(local["backfill_start"])
            if local.get("jwt_secret"):
                self.jwt_secret = str(local["jwt_secret"])
            if "env" in local:
                cfg_env = str(local["env"])
            if "scheduler_enabled" in local:
                self.scheduler_enabled = bool(local["scheduler_enabled"])
            if "schema_strict" in local:
                self.schema_strict = bool(local["schema_strict"])
            if "backtest_async" in local:
                self.backtest_async = bool(local["backtest_async"])
            if "task_async" in local:
                self.task_async = bool(local["task_async"])
            if "task_workers" in local:
                self.task_workers = int(local["task_workers"])
            if "bulk_daily_bars" in local:
                self.bulk_daily_bars = bool(local["bulk_daily_bars"])

        # QUANT_ENV 优先于 config.toml,便于 systemd 注入而无需改配置文件
        self.env = normalize_env(os.environ.get("QUANT_ENV") or cfg_env)

        if not self.database_url:
            raise ValueError(
                "缺少数据库配置: 请在根 config.toml 配置 [server].database_url, "
                "或在 quant/config.toml 配置 [quant].database_url"
            )
        if not self.jwt_secret:
            raise ValueError(
                "缺少 JWT 密钥配置: 请在根 config.toml 配置 [server].jwt_secret, "
                "或在 quant/config.toml 配置 [quant].jwt_secret"
            )

    @property
    def is_production(self) -> bool:
        return self.env == ENV_PROD


settings = Settings()
