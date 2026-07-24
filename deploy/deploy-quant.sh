#!/usr/bin/env bash
# quant 量化系统部署脚本(与 hank-server 同一台服务器)
#
# 用法:
#   ./deploy/deploy-quant.sh             # 全量部署(依赖检查 -> 构建 -> 部署 -> 重启)
#   ./deploy/deploy-quant.sh --skip-deps # 跳过服务器 uv/python 安装(已装过时加速)
#
# 流程:
#   1. 服务器安装 uv (Python 版本由 uv 自动管理, 按 pyproject 解析 3.11~3.13)
#   2. 本地构建 quant/web 前端 (pnpm build)
#   3. rsync quant 源码 + 前端产物到 /opt/hank-quant
#   4. 服务器 uv sync --locked 创建/更新 .venv
#   5. 注册并重启 systemd 服务 hank-quant, 健康检查 :8100/api/health
#
# 服务器上的 config.toml 只在缺失时生成一次(从本地根 config.toml 提取 database_url),
# 之后不会被覆盖。
# 前端由 FastAPI 托管 (web/dist), 浏览器直接访问 http://<服务器>:8100
#
# 注意: uv.lock 记录的 wheel URL 来自锁定时使用的 index。曾因默认锁到
# files.pythonhosted.org 导致服务器下载极慢(镜像环境变量不生效,uv 只从镜像
# 取元数据, 文件仍按 lock 里的 URL 下载)。现已在 pyproject [tool.uv.index]
# 固定阿里云镜像, 改依赖后重新 uv lock 即可保持镜像 URL。

set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
REMOTE_APP="/opt/hank-quant"        # 部署目录
SERVICE_NAME="hank-quant"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SKIP_DEPS=0
[[ "${1:-}" == "--skip-deps" ]] && SKIP_DEPS=1

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

# ---------- 1. 服务器依赖: uv ----------
if [[ $SKIP_DEPS -eq 0 ]]; then
  log "安装服务器依赖 (uv)..."
  ssh "$SSH_HOST" bash -s <<'REMOTE'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl ca-certificates
if [[ ! -x "$HOME/.local/bin/uv" ]]; then
  curl -LsSf https://astral.sh/uv/install.sh | sh
fi
"$HOME/.local/bin/uv" --version
REMOTE
else
  log "跳过依赖安装"
fi

# ---------- 2. 本地构建前端 ----------
log "本地构建 quant/web 前端..."
cd "$PROJECT_ROOT/quant/web"
pnpm install --frozen-lockfile
pnpm build
cd "$PROJECT_ROOT"

# ---------- 3. 同步源码与前端产物 ----------
log "同步 quant 到服务器 $REMOTE_APP ..."
ssh "$SSH_HOST" "mkdir -p $REMOTE_APP"
rsync -az --delete -e ssh \
  --exclude '.venv' \
  --exclude '__pycache__' \
  --exclude '*.pyc' \
  --exclude 'web/node_modules' \
  --exclude 'web/dist' \
  --exclude 'config.toml' \
  "$PROJECT_ROOT/quant/" "$SSH_HOST:$REMOTE_APP/"
# 前端产物单独同步(上面排除了, 避免 --delete 误删后又全量重传)
rsync -az --delete -e ssh \
  "$PROJECT_ROOT/quant/web/dist/" "$SSH_HOST:$REMOTE_APP/web/dist/"

# ---------- 4. 服务器创建/更新 venv ----------
# uv.lock 已固定阿里云镜像的文件 URL(见 pyproject [tool.uv.index]),
# sync 直接从镜像下载, 不会再走 pypi.org
log "服务器 uv sync --frozen (首次需下载 Python, 较慢)..."
ssh "$SSH_HOST" "cd $REMOTE_APP && UV_HTTP_TIMEOUT=300 \$HOME/.local/bin/uv sync --frozen"

# ---------- 5. config.toml: 只在服务器缺失时生成, 绝不覆盖 ----------
if ssh "$SSH_HOST" "[[ ! -f $REMOTE_APP/config.toml ]]"; then
  log "生成服务器 config.toml (仅首次, 从本地根 config.toml 提取 database_url)..."
  DB_URL=$(python3 -c "
import tomllib
with open('$PROJECT_ROOT/config.toml', 'rb') as f:
    print(tomllib.load(f)['server']['database_url'])
")
  ssh "$SSH_HOST" "printf '[quant]\ndatabase_url = \"%s\"\n' '$DB_URL' > $REMOTE_APP/config.toml && chmod 600 $REMOTE_APP/config.toml"
else
  log "服务器已有 config.toml, 跳过 (如需更新请手动 ssh 修改 $REMOTE_APP/config.toml)"
fi

# ---------- 6. systemd 服务 ----------
log "注册并重启 systemd 服务..."
scp -q "$PROJECT_ROOT/deploy/hank-quant.service" "$SSH_HOST:/etc/systemd/system/$SERVICE_NAME.service"
ssh "$SSH_HOST" bash -s <<REMOTE
set -euo pipefail
systemctl daemon-reload
systemctl enable $SERVICE_NAME
systemctl restart $SERVICE_NAME
sleep 2
systemctl is-active $SERVICE_NAME
REMOTE

# ---------- 7. 健康检查 (首次启动需连数据库建表, 重试最多 30s) ----------
log "健康检查..."
ssh "$SSH_HOST" bash -s <<'REMOTE'
set -euo pipefail
for i in $(seq 1 10); do
  if curl -fsS -m 3 http://127.0.0.1:8100/api/health; then
    echo
    exit 0
  fi
  sleep 3
done
echo "ERROR: 健康检查失败, 查看 journalctl -u hank-quant" >&2
exit 1
REMOTE

log "部署完成 ✔  访问 http://<服务器>:8100  (systemctl status $SERVICE_NAME / journalctl -u $SERVICE_NAME 查看状态)"
