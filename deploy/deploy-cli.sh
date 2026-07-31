#!/usr/bin/env bash
# hank-cli 部署脚本: 在服务器上构建 hank-cli 并注册为 systemd 服务
#
# 用法:
#   ./deploy/deploy-cli.sh
#
# 前置条件: 服务器已跑过 deploy.sh (有 rust 工具链和 /opt/hank)
# 流程:
#   1. rsync cli/ 源码到服务器 (排除 target/)
#   2. 服务器上 cargo build --release
#   3. 安装二进制到 /opt/hank/hank-cli
#   4. hank-cli.toml 只在缺失时从本地 ~/.hank-cli/config.toml 上传一次, 之后不覆盖
#   5. 注册并重启 systemd 服务 hank-cli
#
# 注意: CLI 在服务器上连接的是本机的 hank-server (http://127.0.0.1:3000),
#       账号需是线上 hank-server 里有 client 登录权限的用户.

set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
REMOTE_SRC="/root/hank-build"     # 服务器上的 SSH 应急构建目录
REMOTE_APP="/opt/hank-cli"        # release 部署目录
SERVICE_NAME="hank-cli"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELEASE_ID="manual-$(git -C "$PROJECT_ROOT" rev-parse --short=12 HEAD)-$(date +%Y%m%d%H%M%S)"
REMOTE_RELEASE="$REMOTE_APP/releases/$RELEASE_ID"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

# ---------- 1. 同步 cli 源码 ----------
log "同步 cli 源码到服务器..."
ssh "$SSH_HOST" "mkdir -p $REMOTE_SRC"
rsync -az --delete --exclude 'target/' -e ssh \
  "$PROJECT_ROOT/cli" \
  "$SSH_HOST:$REMOTE_SRC/"

# ---------- 2. 服务器上构建 ----------
log "服务器上 cargo build --release (cli 是独立项目, 首次较慢)..."
ssh "$SSH_HOST" "cd $REMOTE_SRC/cli && \$HOME/.cargo/bin/cargo build --release"

# ---------- 3. 安装二进制 ----------
log "安装二进制到 $REMOTE_RELEASE/hank-cli ..."
ssh "$SSH_HOST" "mkdir -p '$REMOTE_RELEASE' && install -m 755 '$REMOTE_SRC/cli/target/release/hank-cli' '$REMOTE_RELEASE/hank-cli'"

# ---------- 4. 配置: 只在缺失时上传, 绝不覆盖 ----------
if ssh "$SSH_HOST" "[[ ! -f $REMOTE_APP/hank-cli.toml ]]"; then
  LOCAL_CFG="$HOME/.hank-cli/config.toml"
  if [[ -f "$LOCAL_CFG" ]]; then
    # 占位符账号直接上传会让服务登录失败死循环, 拦下来
    if grep -q 'your-username\|your-password' "$LOCAL_CFG"; then
      echo "ERROR: 本地 $LOCAL_CFG 还是占位符账号, 请先填入线上 hank-server 的真实账号再部署" >&2
      exit 1
    fi
    log "上传本地 ~/.hank-cli/config.toml -> $REMOTE_APP/hank-cli.toml (仅首次)..."
    scp -q "$LOCAL_CFG" "$SSH_HOST:$REMOTE_APP/hank-cli.toml"
    ssh "$SSH_HOST" "chown hank:hank '$REMOTE_APP/hank-cli.toml' && chmod 600 '$REMOTE_APP/hank-cli.toml'"
    # client_id 是节点身份, 线上应是独立节点: 删掉让 CLI 在服务器上重新生成
    ssh "$SSH_HOST" "sed -i '/^client_id\s*=/d' $REMOTE_APP/hank-cli.toml"
    log "提示: 配置里的 server 指向 $(grep '^server' "$LOCAL_CFG" | head -1), 线上应为本机 http://127.0.0.1:3000"
  else
    echo "ERROR: 服务器上没有 hank-cli.toml, 本地也没有 ~/.hank-cli/config.toml" >&2
    echo "请在服务器上参考 cli/config.example.toml 创建 $REMOTE_APP/hank-cli.toml 后重跑本脚本" >&2
    exit 1
  fi
else
  log "服务器已有 hank-cli.toml, 跳过 (如需更新请手动 scp)"
  ssh "$SSH_HOST" "chown hank:hank '$REMOTE_APP/hank-cli.toml' && chmod 600 '$REMOTE_APP/hank-cli.toml'"
fi

ssh "$SSH_HOST" bash -s -- "$REMOTE_RELEASE" "$REMOTE_APP" "$RELEASE_ID" <<'REMOTE'
set -euo pipefail
release="$1"
app="$2"
release_id="$3"
previous=""
if [[ -L "$app/current" ]]; then
  previous="$(readlink -f "$app/current" 2>/dev/null || true)"
fi
[[ -z "$previous" ]] || ln -sfn "$previous" "$app/previous"
ln -s "$release" "$app/current.tmp.$release_id"
mv -Tf "$app/current.tmp.$release_id" "$app/current"
REMOTE

# ---------- 5. systemd 服务 ----------
log "注册并重启 systemd 服务..."
scp -q "$PROJECT_ROOT/deploy/hank-cli.service" "$SSH_HOST:/etc/systemd/system/$SERVICE_NAME.service"
ssh "$SSH_HOST" bash -s <<REMOTE
set -euo pipefail
systemctl daemon-reload
systemctl enable $SERVICE_NAME
systemctl restart $SERVICE_NAME
sleep 3
systemctl is-active $SERVICE_NAME
REMOTE

# ---------- 6. 启动检查: 最近日志应出现注册成功 ----------
log "启动检查..."
ssh "$SSH_HOST" bash -s <<'REMOTE'
set -euo pipefail
if journalctl -u hank-cli --since "-30s" --no-pager | grep -qE "注册|registration|poll"; then
  echo "hank-cli 已启动并完成注册"
else
  echo "WARNING: 未在日志中看到注册成功, 请 journalctl -u hank-cli -e 检查 (常见原因: 账号密码错误/无 client 权限)" >&2
fi
REMOTE

log "部署完成 ✔  (systemctl status $SERVICE_NAME / journalctl -u $SERVICE_NAME -f 查看状态)"
log "到 admin 后台 clients 列表确认线上节点 online"
