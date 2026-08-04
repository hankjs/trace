#!/usr/bin/env bash
# Hank server + admin 部署：本地交叉编译，只把产物推到服务器。
#
# 用法: ./deploy/deploy.sh
#
# 流程:
#   1. 本地 cargo zigbuild 交叉编译 hank-server → x86_64 glibc 2.27
#   2. 本地 pnpm build 出 admin/dist
#   3. scp/rsync 产物到 /opt/hank/releases/<release-id>
#   4. 原子切换 current 软链（previous 保留上一版可回退）
#   5. 重启 systemd 服务并做健康检查
#
# 为什么本地编译：服务器是 Ubuntu 18.04（glibc 2.27），远端 cargo build 要在生产机上
# 装 rustup 与 build-essential，且构建期间抢占线上 CPU。zig 自带多版本 glibc 符号桩，
# 本地 arm64 直接产出 2.27 兼容的 x86_64 二进制。
#
# 前置依赖（本机一次性）:
#   brew install zig && cargo install cargo-zigbuild
#   rustup target add x86_64-unknown-linux-gnu
#
# 运行期可选依赖（脚本不装，按需手动）:
#   - /snap 网页截图: apt install -y chromium，并在 config.toml 配 chrome_path
#
# 服务器上的 config.toml 只在缺失时上传一次，之后不会被覆盖。

set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
# 保留的历史发布数量。每个发布约 66MB，线上 /opt 只有个位数 GB 余量，
# 不清理会慢性写满磁盘——磁盘满会同时搞掉部署和正在跑的服务。
KEEP_RELEASES="${KEEP_RELEASES:-5}"
REMOTE_APP="/opt/hank"
SERVICE_NAME="hank-server"
TARGET="x86_64-unknown-linux-gnu"
# 目标机 glibc 版本。升级服务器 OS 后同步调高，否则白付兼容性代价。
GLIBC="2.27"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GIT_SHA="$(git -C "$PROJECT_ROOT" rev-parse HEAD)"
RELEASE_ID="manual-${GIT_SHA:0:12}-$(date +%Y%m%d%H%M%S)"
REMOTE_RELEASE="$REMOTE_APP/releases/$RELEASE_ID"
BINARY="$PROJECT_ROOT/target/$TARGET/release/hank-server"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

command -v cargo-zigbuild >/dev/null || {
  echo "ERROR: 缺少 cargo-zigbuild，请先 brew install zig && cargo install cargo-zigbuild" >&2
  exit 1
}

# ---------- 1. 本地交叉编译 ----------
log "本地交叉编译 hank-server ($TARGET.$GLIBC) ..."
cd "$PROJECT_ROOT"
cargo zigbuild --release -p hank-server --target "$TARGET.$GLIBC"
[[ -x "$BINARY" ]] || { echo "ERROR: 未找到构建产物 $BINARY" >&2; exit 1; }

# 交叉编译最容易在这里翻车：链到了本机 dylib 就会在服务器上起不来。
if command -v objdump >/dev/null && objdump -p "$BINARY" 2>/dev/null | grep -q 'NEEDED.*libssl'; then
  echo "ERROR: 二进制仍依赖 libssl，检查是否有 crate 引入了 native-tls" >&2
  exit 1
fi

# ---------- 2. 本地构建 admin ----------
log "本地构建 admin 前端 ..."
cd "$PROJECT_ROOT/admin"
pnpm install --frozen-lockfile
pnpm build
cd "$PROJECT_ROOT"

# ---------- 3. 推送产物 ----------
log "推送产物到 $REMOTE_RELEASE ..."
ssh "$SSH_HOST" "mkdir -p '$REMOTE_RELEASE/admin' '$REMOTE_APP/logs' && chown -R hank:hank '$REMOTE_APP/logs'"
scp -q "$BINARY" "$SSH_HOST:$REMOTE_RELEASE/hank-server.new"
ssh "$SSH_HOST" "install -m 755 '$REMOTE_RELEASE/hank-server.new' '$REMOTE_RELEASE/hank-server' && rm -f '$REMOTE_RELEASE/hank-server.new'"
rsync -az --delete -e ssh "$PROJECT_ROOT/admin/dist/" "$SSH_HOST:$REMOTE_RELEASE/admin/dist/"
ssh "$SSH_HOST" "ln -sfn '$REMOTE_APP/logs' '$REMOTE_RELEASE/logs'"

# config.toml: 只在服务器缺失时上传，绝不覆盖（线上配置与本地长期不同步）
if ssh "$SSH_HOST" "[[ ! -f $REMOTE_APP/config.toml ]]"; then
  [[ -f "$PROJECT_ROOT/config.toml" ]] || {
    echo "ERROR: 服务器和本地都没有 config.toml，请先从 config.example.toml 创建" >&2
    exit 1
  }
  log "上传 config.toml（仅首次）..."
  scp -q "$PROJECT_ROOT/config.toml" "$SSH_HOST:$REMOTE_APP/config.toml"
fi
ssh "$SSH_HOST" "chown root:hank '$REMOTE_APP/config.toml' && chmod 640 '$REMOTE_APP/config.toml'"

# ---------- 4. 原子切换 ----------
log "切换 current 软链 ..."
ssh "$SSH_HOST" bash -s -- "$REMOTE_RELEASE" "$REMOTE_APP" "$RELEASE_ID" <<'REMOTE'
set -euo pipefail
release="$1"; app="$2"; release_id="$3"
previous=""
if [[ -L "$app/current" ]]; then
  previous="$(readlink -f "$app/current" 2>/dev/null || true)"
fi
[[ -z "$previous" ]] || ln -sfn "$previous" "$app/previous"
# 同目录临时链接 + mv -T：切换是原子的，不会出现 current 短暂不存在的窗口
ln -s "$release" "$app/current.tmp.$release_id"
mv -Tf "$app/current.tmp.$release_id" "$app/current"
REMOTE

# ---------- 5. 重启与健康检查 ----------
log "重启 systemd 服务 ..."
scp -q "$PROJECT_ROOT/deploy/hank-server.service" "$SSH_HOST:/etc/systemd/system/$SERVICE_NAME.service"
ssh "$SSH_HOST" "systemctl daemon-reload && systemctl enable -q $SERVICE_NAME && systemctl restart $SERVICE_NAME"

log "健康检查 ..."
ssh "$SSH_HOST" bash -s <<'REMOTE'
set -euo pipefail
# server 要先连上数据库才会应答，给 30s
for _ in $(seq 1 10); do
  if curl -fsS -m 3 http://127.0.0.1:3000/api/health; then
    echo
    exit 0
  fi
  sleep 3
done
echo "ERROR: 健康检查失败，查看 journalctl -u hank-server" >&2
exit 1
REMOTE

# ---------- 6. 清理历史发布 ----------
# 放在健康检查之后：新版本没起来时不动任何旧发布，回退路径必须保持完整。
log "清理历史发布（保留最近 $KEEP_RELEASES 个）..."
ssh "$SSH_HOST" bash -s -- "$REMOTE_APP" "$KEEP_RELEASES" <<'REMOTE'
set -euo pipefail
app="$1"; keep="$2"
cd "$app/releases"
# current / previous 指向的目录永不删除，无论排在第几位
cur="$(readlink -f "$app/current" 2>/dev/null || true)"
prev="$(readlink -f "$app/previous" 2>/dev/null || true)"
# 按发布 ID 末尾的时间戳倒序，不用 mtime：rsync 与软链操作都会改 mtime，
# 排序会漂。同时只认 deploy.sh 生成的 ID 格式，手工建的目录一律不碰。
candidates="$(ls -1 | grep -E '^[a-z]+-[0-9a-f]+-[0-9]{14}$' || true)"
echo "$candidates" | sed '/^$/d' | sort -t- -k3,3r | tail -n "+$((keep + 1))" | while read -r dir; do
  full="$(readlink -f "$dir")"
  if [ "$full" = "$cur" ] || [ "$full" = "$prev" ]; then
    echo "keep (linked) $dir"
    continue
  fi
  rm -rf -- "$dir"
  echo "removed $dir"
done
df -h "$app" | tail -1
REMOTE

log "部署完成 ✔  回退: ssh $SSH_HOST 'ln -sfn \$(readlink -f $REMOTE_APP/previous) $REMOTE_APP/current && systemctl restart $SERVICE_NAME'"
