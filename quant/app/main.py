"""FastAPI 入口:启动时校验 Alembic 版本 + 抢到互斥锁的实例启动定时任务。

建表与改表**不再在启动时进行**。原先的两种做法都有问题:
声明式建表从不 ALTER 既有表,模型改动会被静默忽略;
手写 ALTER 每条语句一个事务,多副本同时启动会并发 DDL 竞态。

现由 Alembic 管理,部署流程需在启动前执行 `alembic upgrade head`,
启动时只校验版本(见 app/migrations.py)。
"""
from __future__ import annotations

import logging
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import Depends, FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, JSONResponse
from fastapi.staticfiles import StaticFiles

from .api import (admin, auth, backtest, catalog, experiments, factors, market,
                  pools, portfolio, research_plans, selection, settings as
                  settings_api, signals, strategies, tasks, watchlist)
from .auth import require_admin, require_client
from .config import settings
from .db import engine
from . import models  # noqa: F401 - 确保模型注册到 Base.metadata
from .migrations import check_schema_version
from .scheduler import start_scheduler, stop_scheduler
from .scheduler_lock import acquire_scheduler_slot, release_scheduler_slot
from .tasks import recover_tasks, shutdown_tasks
from .a2a import mount_a2a_routes

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logger = logging.getLogger(__name__)

_a2a_handler = None


@asynccontextmanager
async def lifespan(app: FastAPI):
    global _a2a_handler
    check_schema_version(engine)
    logger.info(
        "quant 启动: env=%s scheduler_enabled=%s",
        settings.env, settings.scheduler_enabled,
    )
    # 仅 production 且抢到互斥锁的实例运行定时任务
    # (dev 只跑业务 API;多副本时避免重复抓取,REVIEW 问题 6)
    owns_scheduler = acquire_scheduler_slot()
    if owns_scheduler:
        start_scheduler()
    # 异步任务恢复:中断的 running 标记失败,pending 重新派发
    recover_tasks()
    yield
    shutdown_tasks()
    if _a2a_handler is not None:
        await _a2a_handler.aclose()
    if owns_scheduler:
        stop_scheduler()
        release_scheduler_slot()


app = FastAPI(title="quant - A股日频量化信息系统", lifespan=lifespan)

app.add_middleware(
    CORSMiddleware,
    allow_origins=settings.cors_origins,
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

app.include_router(auth.router)

# 业务接口全部要求登录;/api/auth/login 与 /api/health 保持公开
_auth = [Depends(require_client)]
app.include_router(catalog.router, dependencies=_auth)
app.include_router(market.router, dependencies=_auth)
app.include_router(watchlist.router, dependencies=_auth)
app.include_router(settings_api.router, dependencies=_auth)
app.include_router(pools.router, dependencies=_auth)
app.include_router(strategies.router, dependencies=_auth)
app.include_router(signals.router, dependencies=_auth)
app.include_router(research_plans.router, dependencies=_auth)
app.include_router(portfolio.router, dependencies=_auth)
app.include_router(backtest.router, dependencies=_auth)
app.include_router(backtest.plural_router, dependencies=_auth)
app.include_router(experiments.router, dependencies=_auth)
app.include_router(selection.router, dependencies=_auth)
app.include_router(factors.router, dependencies=_auth)
app.include_router(tasks.router, dependencies=_auth)
app.include_router(admin.router, dependencies=[Depends(require_admin)])

# A2A 路由：Card 公开，/a2a JSON-RPC 由 context builder 做 JWT 鉴权
_a2a_handler = mount_a2a_routes(app)


@app.get("/api/health")
def health():
    return {"status": "ok", "env": settings.env}


# 生产模式:若存在前端构建产物(quant/web/dist),由本进程直接托管,
# 部署时只需一个服务。开发模式走 vite dev server,此分支不生效。
WEB_DIST = Path(__file__).resolve().parent.parent / "web" / "dist"
if WEB_DIST.exists():

    @app.exception_handler(404)
    async def spa_fallback(request: Request, exc):
        # SPA 前端路由(history 模式)刷新时回退到 index.html;API 404 原样返回
        if request.url.path.startswith("/api/"):
            return JSONResponse({"detail": "Not Found"}, status_code=404)
        return FileResponse(WEB_DIST / "index.html")

    app.mount("/", StaticFiles(directory=WEB_DIST, html=True), name="web")
