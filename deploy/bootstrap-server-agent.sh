#!/usr/bin/env bash
# wananyun 首次初始化：创建受限运行用户、生产基线仓库、release 目录、
# root 部署 helper、sudoers、systemd 与 nginx。日常迭代完成后不再需要本脚本。
set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_URL="${REPO_URL:-$(git -C "$PROJECT_ROOT" remote get-url origin)}"
BOOTSTRAP_REF="${BOOTSTRAP_REF:-$(git -C "$PROJECT_ROOT" rev-parse HEAD)}"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

if [[ -n "$(git -C "$PROJECT_ROOT" status --porcelain)" ]]; then
  echo "ERROR: bootstrap 前请先提交当前改动，生产基线必须对应一个 Git commit" >&2
  exit 1
fi

log "上传 server Agent 基础设施到 $SSH_HOST ..."
REMOTE_STAGE="$(ssh "$SSH_HOST" 'mktemp -d /tmp/hank-bootstrap.XXXXXX')"
case "$REMOTE_STAGE" in
  /tmp/hank-bootstrap.*) ;;
  *) echo "ERROR: 远端临时目录异常: $REMOTE_STAGE" >&2; exit 1 ;;
esac
cleanup() { ssh "$SSH_HOST" "rm -rf -- '$REMOTE_STAGE'" >/dev/null 2>&1 || true; }
trap cleanup EXIT

scp -q \
  "$PROJECT_ROOT/deploy/hank-deploy" \
  "$PROJECT_ROOT/deploy/hank-server.service" \
  "$PROJECT_ROOT/deploy/hank-cli.service" \
  "$PROJECT_ROOT/deploy/hank-quant.service" \
  "$PROJECT_ROOT/deploy/quant-slidev.nginx" \
  "$PROJECT_ROOT/deploy/hank-docs.nginx" \
  "$SSH_HOST:$REMOTE_STAGE/"
if [[ -f "$PROJECT_ROOT/config.toml" ]]; then
  scp -q "$PROJECT_ROOT/config.toml" "$SSH_HOST:$REMOTE_STAGE/config.toml"
fi

log "初始化用户、工具链、仓库与服务目录 ..."
ssh "$SSH_HOST" bash -s -- "$REMOTE_STAGE" "$REPO_URL" "$BOOTSTRAP_REF" <<'REMOTE'
set -euo pipefail

STAGE="$1"
REPO_URL="$2"
BOOTSTRAP_REF="$3"
if [[ $EUID -ne 0 ]]; then
  echo "ERROR: 远端 bootstrap 必须以 root 执行" >&2
  exit 1
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq \
  build-essential ca-certificates curl git libssl-dev nginx npm pkg-config \
  python3 rsync sudo util-linux

if ! getent group hank-workspace >/dev/null 2>&1; then
  groupadd --system hank-workspace
fi
if ! id hank >/dev/null 2>&1; then
  useradd --create-home --shell /bin/bash hank
fi
if ! id hank-build >/dev/null 2>&1; then
  useradd --create-home --shell /usr/sbin/nologin hank-build
fi
usermod -a -G hank-workspace hank
usermod -a -G hank-workspace hank-build

node_major="$(node --version 2>/dev/null | sed -E 's/^v([0-9]+).*/\1/' || true)"
if [[ -z "$node_major" || "$node_major" -lt 20 ]]; then
  curl -fsSL https://deb.nodesource.com/setup_22.x -o /tmp/hank-nodesource.sh
  bash /tmp/hank-nodesource.sh
  rm -f /tmp/hank-nodesource.sh
  apt-get install -y -qq nodejs
fi
npm install --global pnpm@10.26.2

if [[ ! -x /home/hank-build/.cargo/bin/cargo ]]; then
  runuser --user hank-build -- env HOME=/home/hank-build bash -c \
    'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/hank-rustup.sh && sh /tmp/hank-rustup.sh -y --default-toolchain stable --profile minimal && rm -f /tmp/hank-rustup.sh'
fi
install -d -o hank-build -g hank-build -m 755 /home/hank-build/.cargo
if [[ ! -f /home/hank-build/.cargo/config.toml ]]; then
  install -o hank-build -g hank-build -m 644 /dev/null /home/hank-build/.cargo/config.toml
  cat > /home/hank-build/.cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = 'rsproxy-sparse'
[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
[net]
git-fetch-with-cli = true
EOF
  chown hank-build:hank-build /home/hank-build/.cargo/config.toml
fi

if [[ ! -x /home/hank-build/.local/bin/uv ]]; then
  runuser --user hank-build -- env HOME=/home/hank-build bash -c \
    'curl -LsSf https://astral.sh/uv/install.sh -o /tmp/hank-uv.sh && sh /tmp/hank-uv.sh && rm -f /tmp/hank-uv.sh'
fi
if [[ ! -x /home/hank-build/.cargo/bin/mdbook ]]; then
  runuser --user hank-build -- env HOME=/home/hank-build PATH=/home/hank-build/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
    cargo install mdbook --locked
fi

install -d -o root -g root -m 755 \
  /opt/hank /opt/hank/releases \
  /opt/hank-cli /opt/hank-cli/releases \
  /opt/hank-quant /opt/hank-quant/releases \
  /opt/hank-quant-slidev /opt/hank-quant-slidev/releases \
  /opt/hank-docs /opt/hank-docs/releases
install -d -o hank -g hank -m 2700 /opt/hank/deploy-jobs
install -d -o hank -g hank -m 755 /opt/hank/logs
install -d -o hank -g hank-workspace -m 2770 /opt/hank-worktrees

if [[ ! -d /opt/hank-src/.git ]]; then
  runuser --user hank -- git clone "$REPO_URL" /opt/hank-src
fi
runuser --user hank -- git -C /opt/hank-src fetch --prune origin
if ! runuser --user hank -- git -C /opt/hank-src cat-file -e "$BOOTSTRAP_REF^{commit}"; then
  echo "ERROR: BOOTSTRAP_REF=$BOOTSTRAP_REF 不在远端仓库，请先 push" >&2
  exit 1
fi
BASE_SHA="$(runuser --user hank -- git -C /opt/hank-src rev-parse "$BOOTSTRAP_REF^{commit}")"
if ! runuser --user hank -- git -C /opt/hank-src rev-parse --verify trace-production >/dev/null 2>&1; then
  runuser --user hank -- git -C /opt/hank-src update-ref refs/heads/trace-production "$BASE_SHA"
fi
chown -R hank:hank-workspace /opt/hank-src /opt/hank-worktrees
# 构建用户只需读取生产仓库；公共 config、hooks 和 trace-production ref 不可写。
chmod -R u+rwX,g+rX,o-rwx /opt/hank-src
chmod -R u+rwX,g+rwX,o-rwx /opt/hank-worktrees
find /opt/hank-src /opt/hank-worktrees -type d -exec chmod g+s {} +

# Linked worktree 只开放不可变对象库、话题分支和各 worktree 元数据。
# refs/heads 根目录保持不可写，因此 hank-build 不能创建或替换 trace-production.lock。
GIT_COMMON=/opt/hank-src/.git
for shared_path in \
  "$GIT_COMMON/objects" \
  "$GIT_COMMON/worktrees" \
  "$GIT_COMMON/refs/heads/feishu" \
  "$GIT_COMMON/logs/refs/heads/feishu"; do
  install -d -o hank -g hank-workspace -m 2770 "$shared_path"
  chgrp -R hank-workspace "$shared_path"
  chmod -R g+rwX "$shared_path"
  find "$shared_path" -type d -exec chmod g+s {} +
done
runuser --user hank -- git -C /opt/hank-src config --unset-all core.hooksPath || true
# worktree 由 hank 创建、Git 工具由 hank-build 执行。仅信任话题 worktree 根，
# 避免 Git 的 dubious ownership 检查阻断 add/commit 等正常操作。
runuser --user hank-build -- env HOME=/home/hank-build \
  git config --global --replace-all safe.directory '/opt/hank-worktrees/*'

install -o root -g root -m 755 "$STAGE/hank-deploy" /usr/local/libexec/hank-deploy
cat > /etc/sudoers.d/hank-deploy <<'EOF'
hank ALL=(root) NOPASSWD: /usr/local/libexec/hank-deploy *
hank ALL=(hank-build) NOPASSWD: ALL
EOF
chmod 440 /etc/sudoers.d/hank-deploy
visudo -cf /etc/sudoers.d/hank-deploy

install -o root -g root -m 644 "$STAGE/hank-server.service" /etc/systemd/system/hank-server.service
install -o root -g root -m 644 "$STAGE/hank-cli.service" /etc/systemd/system/hank-cli.service
install -o root -g root -m 644 "$STAGE/hank-quant.service" /etc/systemd/system/hank-quant.service
install -o root -g root -m 644 "$STAGE/quant-slidev.nginx" /etc/nginx/sites-enabled/quant-slidev
install -o root -g root -m 644 "$STAGE/hank-docs.nginx" /etc/nginx/sites-enabled/hank-docs

if [[ ! -f /opt/hank/config.toml && -f "$STAGE/config.toml" ]]; then
  install -o root -g hank -m 640 "$STAGE/config.toml" /opt/hank/config.toml
fi
if [[ ! -f /opt/hank/config.toml ]]; then
  echo "ERROR: 远端和本地都缺少 config.toml" >&2
  exit 1
fi
if ! grep -q '^\[server_agent\]' /opt/hank/config.toml; then
  cat >> /opt/hank/config.toml <<'EOF'

[server_agent]
enabled = true
repository_root = "/opt/hank-src"
worktrees_root = "/opt/hank-worktrees"
base_ref = "trace-production"
deploy_jobs_dir = "/opt/hank/deploy-jobs"
deploy_helper = "/usr/local/libexec/hank-deploy"
execution_user = "hank-build"
deploy_use_sudo = true
approval_ttl_secs = 600
EOF
else
  python3 - /opt/hank/config.toml <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
desired = {
    "enabled": "enabled = true\n",
    "repository_root": 'repository_root = "/opt/hank-src"\n',
    "worktrees_root": 'worktrees_root = "/opt/hank-worktrees"\n',
    "base_ref": 'base_ref = "trace-production"\n',
    "deploy_jobs_dir": 'deploy_jobs_dir = "/opt/hank/deploy-jobs"\n',
    "deploy_helper": 'deploy_helper = "/usr/local/libexec/hank-deploy"\n',
    "execution_user": 'execution_user = "hank-build"\n',
    "deploy_use_sudo": "deploy_use_sudo = true\n",
}
start = next(i for i, line in enumerate(lines) if line.strip() == "[server_agent]")
end = next(
    (i for i in range(start + 1, len(lines)) if lines[i].lstrip().startswith("[")),
    len(lines),
)
found = set()
for index in range(start + 1, end):
    stripped = lines[index].strip()
    if not stripped or stripped.startswith("#") or "=" not in stripped:
        continue
    key = stripped.split("=", 1)[0].strip()
    if key in desired:
        lines[index] = desired[key]
        found.add(key)
missing = [value for key, value in desired.items() if key not in found]
lines[end:end] = missing
path.write_text("".join(lines), encoding="utf-8")
PY
fi
chown root:hank /opt/hank/config.toml
chmod 640 /opt/hank/config.toml

# 兼容旧布局：保留当前可执行版本，后续手工部署会写入正式 release。
if [[ ! -L /opt/hank/current && -x /opt/hank/hank-server ]]; then
  mkdir -p "/opt/hank/releases/bootstrap/admin"
  install -m 755 /opt/hank/hank-server /opt/hank/releases/bootstrap/hank-server
  if [[ -d /opt/hank/admin/dist ]]; then
    rsync -a --delete /opt/hank/admin/dist/ /opt/hank/releases/bootstrap/admin/dist/
  fi
  ln -s /opt/hank/logs /opt/hank/releases/bootstrap/logs
  ln -s /opt/hank/releases/bootstrap /opt/hank/current
fi
if [[ ! -L /opt/hank-cli/current && -x /opt/hank/hank-cli ]]; then
  mkdir -p /opt/hank-cli/releases/bootstrap
  install -m 755 /opt/hank/hank-cli /opt/hank-cli/releases/bootstrap/hank-cli
  ln -s /opt/hank-cli/releases/bootstrap /opt/hank-cli/current
fi
if [[ ! -f /opt/hank-cli/hank-cli.toml && -f /opt/hank/hank-cli.toml ]]; then
  install -o hank -g hank -m 600 /opt/hank/hank-cli.toml /opt/hank-cli/hank-cli.toml
fi

systemctl daemon-reload
nginx -t
systemctl reload nginx
if [[ -x /opt/hank/current/hank-server ]]; then
  systemctl enable --now hank-server
fi
if [[ -x /opt/hank-cli/current/hank-cli && -f /opt/hank-cli/hank-cli.toml ]]; then
  systemctl enable --now hank-cli
fi
REMOTE

log "bootstrap 完成。下一步运行 make deploy / deploy-cli / deploy-quant 建立正式 release。"
