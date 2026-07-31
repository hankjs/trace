#!/usr/bin/env bash
# Hank server + admin 自动构建部署脚本
#
# 用法:
#   ./deploy/deploy.sh             # 全量部署(依赖检查 -> 构建 -> 部署 -> 重启)
#   ./deploy/deploy.sh --skip-deps # 跳过 apt/rustup 依赖安装(已装过时加速)
#
# 流程:
#   1. 服务器安装构建依赖 (build-essential / pkg-config / libssl-dev / rustup)
#   2. 本地构建 admin 前端 (pnpm build)
#   3. rsync Rust 源码到服务器, cargo build --release
#   4. 安装二进制 + admin/dist 到 /opt/hank, 注册 systemd 服务并重启
#
# 服务器上的 config.toml 只在缺失时从本地上传一次, 之后不会被覆盖.
#
# 运行期可选依赖(脚本不自动安装, 按需手动装):
#   - /snap 网页截图: apt install -y chromium, 并在 config.toml 配置 chrome_path
#   - /shot 终端截图: 服务器需等宽 CJK 字体(如 apt install -y fonts-noto-cjk), 否则中文缺字形

set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
REMOTE_SRC="/root/hank-build"     # SSH 应急部署的临时构建目录
REMOTE_APP="/opt/hank"            # 部署目录
SERVICE_NAME="hank-server"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GIT_SHA="$(git -C "$PROJECT_ROOT" rev-parse HEAD)"
RELEASE_ID="manual-${GIT_SHA:0:12}-$(date +%Y%m%d%H%M%S)"
REMOTE_RELEASE="$REMOTE_APP/releases/$RELEASE_ID"

SKIP_DEPS=0
[[ "${1:-}" == "--skip-deps" ]] && SKIP_DEPS=1

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

if [[ -n "$(git -C "$PROJECT_ROOT" status --porcelain)" ]]; then
  echo "ERROR: 手工部署前请先提交全部改动，trace-production 必须对应确定的 commit" >&2
  exit 1
fi

log "校验生产 Git 基线可获取 commit $GIT_SHA ..."
ssh "$SSH_HOST" bash -s -- "$GIT_SHA" <<'REMOTE'
set -euo pipefail
sha="$1"
id hank >/dev/null 2>&1 || { echo "ERROR: 请先运行 make bootstrap-server-agent" >&2; exit 1; }
for attempt in 1 2 3; do
  if runuser --user hank -- git -C /opt/hank-src fetch --prune origin; then
    break
  fi
  if [[ $attempt -eq 3 ]]; then
    echo "ERROR: Git fetch 连续失败 3 次" >&2
    exit 1
  fi
  sleep "$((attempt * 2))"
done
runuser --user hank -- git -C /opt/hank-src cat-file -e "$sha^{commit}" || {
  echo "ERROR: 当前 commit 尚未 push 到 origin" >&2
  exit 1
}
REMOTE

# ---------- 1. 服务器构建依赖 ----------
# 国内服务器默认走 rsproxy.cn 镜像加速 rustup/crates; CARGO_MIRROR=none 可关闭
CARGO_MIRROR="${CARGO_MIRROR:-rsproxy}"
if [[ $SKIP_DEPS -eq 0 ]]; then
  log "安装服务器构建依赖 (apt + rustup, 镜像: $CARGO_MIRROR)..."
  ssh "$SSH_HOST" "CARGO_MIRROR='$CARGO_MIRROR'" bash -s <<'REMOTE'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq build-essential pkg-config libssl-dev curl ca-certificates
if [[ ! -x "$HOME/.cargo/bin/cargo" ]]; then
  if [[ "$CARGO_MIRROR" == "rsproxy" ]]; then
    export RUSTUP_DIST_SERVER="https://rsproxy.cn"
    export RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
  fi
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
  sh /tmp/rustup-init.sh -y --default-toolchain stable --profile minimal
  rm -f /tmp/rustup-init.sh
fi
# crates.io 镜像, 加速 cargo build
if [[ "$CARGO_MIRROR" == "rsproxy" && ! -f "$HOME/.cargo/config.toml" ]]; then
  cat > "$HOME/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = 'rsproxy-sparse'
[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
[net]
git-fetch-with-cli = true
EOF
fi
"$HOME/.cargo/bin/cargo" --version
REMOTE
else
  log "跳过依赖安装"
fi

# ---------- 2. 本地构建 admin ----------
log "本地构建 admin 前端..."
cd "$PROJECT_ROOT/admin"
pnpm install --frozen-lockfile
pnpm build
cd "$PROJECT_ROOT"

# ---------- 3. 同步源码并在服务器构建 ----------
log "同步 Rust 源码到服务器..."
ssh "$SSH_HOST" "mkdir -p $REMOTE_SRC"
rsync -az \
  -e ssh \
  "$PROJECT_ROOT/Cargo.toml" "$PROJECT_ROOT/Cargo.lock" \
  "$SSH_HOST:$REMOTE_SRC/"
rsync -az --delete -e ssh \
  "$PROJECT_ROOT/server" "$PROJECT_ROOT/crates" \
  "$SSH_HOST:$REMOTE_SRC/"

log "服务器上 cargo build --release (首次较慢)..."
ssh "$SSH_HOST" "cd $REMOTE_SRC && \$HOME/.cargo/bin/cargo build --release -p hank-server"

# ---------- 4. 部署产物 ----------
log "部署到 $REMOTE_RELEASE ..."
ssh "$SSH_HOST" "mkdir -p '$REMOTE_RELEASE/admin' '$REMOTE_APP/logs'"
# 从服务器构建目录拷贝二进制到部署目录
ssh "$SSH_HOST" "install -m 755 '$REMOTE_SRC/target/release/hank-server' '$REMOTE_RELEASE/hank-server'"
# admin 静态文件
rsync -az --delete -e ssh \
  "$PROJECT_ROOT/admin/dist/" "$SSH_HOST:$REMOTE_RELEASE/admin/dist/"
ssh "$SSH_HOST" "ln -s '$REMOTE_APP/logs' '$REMOTE_RELEASE/logs'"

# config.toml: 只在服务器缺失时上传, 绝不覆盖
if ssh "$SSH_HOST" "[[ ! -f $REMOTE_APP/config.toml ]]"; then
  if [[ -f "$PROJECT_ROOT/config.toml" ]]; then
    log "上传 config.toml (仅首次)..."
    scp -q "$PROJECT_ROOT/config.toml" "$SSH_HOST:$REMOTE_APP/config.toml"
    ssh "$SSH_HOST" "chown root:hank '$REMOTE_APP/config.toml' && chmod 640 '$REMOTE_APP/config.toml'"
  else
    echo "ERROR: 服务器上没有 config.toml, 本地也没有, 请先从 config.example.toml 创建" >&2
    exit 1
  fi
else
  log "服务器已有 config.toml, 跳过 (如需更新请手动 scp)"
  ssh "$SSH_HOST" "chown root:hank '$REMOTE_APP/config.toml' && chmod 640 '$REMOTE_APP/config.toml'"
fi

# current 使用同目录临时链接原子切换，previous 保留上一个可回退版本。
ssh "$SSH_HOST" bash -s -- "$REMOTE_RELEASE" "$REMOTE_APP" "$RELEASE_ID" <<'REMOTE'
set -euo pipefail
release="$1"
app="$2"
release_id="$3"
previous="$(readlink -f "$app/current" 2>/dev/null || true)"
[[ -z "$previous" ]] || ln -sfn "$previous" "$app/previous"
ln -s "$release" "$app/current.tmp.$release_id"
mv -Tf "$app/current.tmp.$release_id" "$app/current"
REMOTE

# ---------- 5. systemd 服务 ----------
log "注册并重启 systemd 服务..."
scp -q "$PROJECT_ROOT/deploy/hank-server.service" "$SSH_HOST:/etc/systemd/system/$SERVICE_NAME.service"
ssh "$SSH_HOST" bash -s <<REMOTE
set -euo pipefail
systemctl daemon-reload
systemctl enable $SERVICE_NAME
systemctl restart $SERVICE_NAME
sleep 2
systemctl is-active $SERVICE_NAME
REMOTE

# ---------- 6. 健康检查 (服务需先连数据库, 重试最多 30s) ----------
log "健康检查..."
ssh "$SSH_HOST" bash -s <<'REMOTE'
set -euo pipefail
for i in $(seq 1 10); do
  if curl -fsS -m 3 http://127.0.0.1:3000/api/health; then
    echo
    exit 0
  fi
  sleep 3
done
echo "ERROR: 健康检查失败, 查看 journalctl -u hank-server" >&2
exit 1
REMOTE

log "推进 trace-production 基线 ..."
ssh "$SSH_HOST" "runuser --user hank -- git -C /opt/hank-src update-ref refs/heads/trace-production '$GIT_SHA'"

log "部署完成 ✔  (systemctl status $SERVICE_NAME / journalctl -u $SERVICE_NAME 查看状态)"
