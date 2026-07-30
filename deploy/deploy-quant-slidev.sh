#!/usr/bin/env bash
# quant 系统介绍 Slidev 部署脚本(与 hank-server / hank-quant 同一台服务器)
#
# 用法:
#   ./deploy/deploy-quant-slidev.sh
#   make deploy-quant-slidev
#
# 流程:
#   1. 本地构建 quant/slidev (pnpm build → dist/)
#   2. rsync 静态产物到 /opt/hank-quant-slidev
#   3. 安装 nginx 站点配置, reload, 健康检查 :3030
#
# 访问: http://<服务器>:3030

set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
REMOTE_APP="/opt/hank-quant-slidev"
NGINX_SITE="quant-slidev"
LISTEN_PORT=3030
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

# ---------- 1. 本地构建 ----------
log "本地构建 quant/slidev..."
cd "$PROJECT_ROOT/quant/slidev"
if [[ ! -d node_modules ]]; then
  pnpm install --frozen-lockfile
else
  # lock 有变时再装; 有 node_modules 时仍尽量用 frozen
  pnpm install --frozen-lockfile
fi
pnpm build
cd "$PROJECT_ROOT"

if [[ ! -f "$PROJECT_ROOT/quant/slidev/dist/index.html" ]]; then
  echo "ERROR: 构建产物缺少 dist/index.html" >&2
  exit 1
fi

# ---------- 2. 同步静态文件 ----------
log "同步 slidev 到服务器 $REMOTE_APP ..."
ssh "$SSH_HOST" "mkdir -p $REMOTE_APP"
rsync -az --delete -e ssh \
  "$PROJECT_ROOT/quant/slidev/dist/" "$SSH_HOST:$REMOTE_APP/"

# ---------- 3. nginx 站点 ----------
log "安装 nginx 站点 ($NGINX_SITE, 端口 $LISTEN_PORT)..."
scp -q "$PROJECT_ROOT/deploy/quant-slidev.nginx" \
  "$SSH_HOST:/etc/nginx/sites-enabled/$NGINX_SITE"
ssh "$SSH_HOST" bash -s <<REMOTE
set -euo pipefail
nginx -t
systemctl reload nginx
REMOTE

# ---------- 4. 健康检查 ----------
log "健康检查..."
ssh "$SSH_HOST" bash -s <<REMOTE
set -euo pipefail
for i in \$(seq 1 5); do
  code=\$(curl -sS -o /dev/null -w '%{http_code}' -m 3 "http://127.0.0.1:$LISTEN_PORT/" || true)
  if [[ "\$code" == "200" ]]; then
    echo "HTTP \$code OK"
    exit 0
  fi
  sleep 1
done
echo "ERROR: 健康检查失败 (期望 HTTP 200, 端口 $LISTEN_PORT)" >&2
exit 1
REMOTE

log "部署完成 ✔  访问 http://<服务器>:$LISTEN_PORT  (SSH_HOST=$SSH_HOST)"
