#!/usr/bin/env bash
# 全新服务器的一次性初始化：只准备运行期需要的东西。
#
# 用法: ./deploy/bootstrap-server.sh
#
# 做什么:
#   - 建 hank 系统用户与 /opt/hank 目录骨架
#   - 装 curl / ca-certificates（健康检查与出网 TLS 需要）
#   - 装 systemd unit
#   - 可选装 nginx 站点（文档站）
#
# 不做什么（构建全在本地，见 deploy/deploy.sh）:
#   - 不装 rustup / cargo / node / pnpm
#   - 不装 bubblewrap 或任何沙箱
#   - 不建 Git 基线仓库、worktree、workspace 目录
#
# 幂等：可以重复执行。之后每次发版只跑 deploy/deploy.sh。

set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

log "初始化用户与目录 ..."
ssh "$SSH_HOST" bash -s <<'REMOTE'
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl ca-certificates

# hank 是纯运行期账号：无登录 shell，无 sudo
id -u hank >/dev/null 2>&1 || useradd --system --create-home --home-dir /home/hank --shell /usr/sbin/nologin hank

mkdir -p /opt/hank/releases /opt/hank/logs
chown -R hank:hank /opt/hank/logs
chown root:hank /opt/hank
chmod 750 /opt/hank

# config.toml 由 deploy.sh 首次上传；这里只保证权限模型正确
if [[ -f /opt/hank/config.toml ]]; then
  chown root:hank /opt/hank/config.toml
  chmod 640 /opt/hank/config.toml
fi

echo "hank uid=$(id -u hank), /opt/hank ready"
REMOTE

log "安装 systemd unit ..."
scp -q "$PROJECT_ROOT/deploy/hank-server.service" "$SSH_HOST:/etc/systemd/system/hank-server.service"
ssh "$SSH_HOST" "systemctl daemon-reload && systemctl enable -q hank-server"

# 文档站点是可选组件，服务器没装 nginx 就跳过，不让 bootstrap 失败
log "配置 nginx 文档站点（可选）..."
if ssh "$SSH_HOST" "command -v nginx >/dev/null"; then
  scp -q "$PROJECT_ROOT/deploy/hank-docs.nginx" "$SSH_HOST:/etc/nginx/sites-available/hank-docs"
  ssh "$SSH_HOST" "ln -sfn /etc/nginx/sites-available/hank-docs /etc/nginx/sites-enabled/hank-docs && nginx -t && systemctl reload nginx"
else
  log "未检测到 nginx，跳过文档站点"
fi

log "初始化完成 ✔  下一步: ./deploy/deploy.sh"
