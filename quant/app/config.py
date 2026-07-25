"""配置加载:读取仓库根目录 config.toml 的 [server].database_url,
并可用 quant/config.toml 的 [quant] 段做覆盖。
"""
from __future__ import annotations

import tomllib
from pathlib import Path

QUANT_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = QUANT_DIR.parent


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
        # 调度器跨进程互斥:多 worker/多副本部署时,即使本开关为 True,
        # 也只有抢到 MySQL GET_LOCK 的那个实例真正运行定时任务(见 scheduler.py)。
        # 置 False 可让某些实例彻底不参与调度(如纯 API worker)。
        self.scheduler_enabled: bool = True

        # quant 自己的覆盖配置(可选)
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
            if "scheduler_enabled" in local:
                self.scheduler_enabled = bool(local["scheduler_enabled"])

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


settings = Settings()
