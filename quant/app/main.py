"""FastAPI 入口:启动时建表(create_all)+ 启动定时任务。"""
from __future__ import annotations

import logging
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import Depends, FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, JSONResponse
from fastapi.staticfiles import StaticFiles

from .api import admin, auth, backtest, market, portfolio, selection, signals, watchlist
from .auth import require_user
from .config import settings
from .db import Base, engine
from . import models  # noqa: F401 - 确保模型注册到 Base.metadata
from .scheduler import start_scheduler, stop_scheduler

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logger = logging.getLogger(__name__)


@asynccontextmanager
async def lifespan(app: FastAPI):
    Base.metadata.create_all(engine)
    logger.info("数据库表(quant_*)已就绪")
    start_scheduler()
    yield
    stop_scheduler()


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
_auth = [Depends(require_user)]
app.include_router(market.router, dependencies=_auth)
app.include_router(watchlist.router, dependencies=_auth)
app.include_router(signals.router, dependencies=_auth)
app.include_router(portfolio.router, dependencies=_auth)
app.include_router(backtest.router, dependencies=_auth)
app.include_router(selection.router, dependencies=_auth)
app.include_router(admin.router, dependencies=_auth)


@app.get("/api/health")
def health():
    return {"status": "ok"}


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
