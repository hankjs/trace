#!/usr/bin/env bash
# wananyun 首次初始化：创建受限运行用户、生产基线仓库、release 目录、
# root 部署 helper、sudoers、systemd 与 nginx。日常迭代完成后不再需要本脚本。
set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_URL="${REPO_URL:-$(git -C "$PROJECT_ROOT" remote get-url origin)}"
BOOTSTRAP_REF="${BOOTSTRAP_REF:-$(git -C "$PROJECT_ROOT" rev-parse HEAD)}"
LOCAL_STAGE=""
LOCAL_BUNDLE=""
REMOTE_STAGE=""

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

cleanup() {
  if [[ "$REMOTE_STAGE" =~ ^/tmp/hank-bootstrap\.[[:alnum:]]+$ ]]; then
    ssh "$SSH_HOST" "rm -rf -- '$REMOTE_STAGE'" >/dev/null 2>&1 || true
  fi
  [[ -z "$LOCAL_BUNDLE" || ! -f "$LOCAL_BUNDLE" ]] || unlink "$LOCAL_BUNDLE"
  [[ -z "$LOCAL_STAGE" ]] || rmdir "$LOCAL_STAGE" 2>/dev/null || true
}
trap cleanup EXIT

if [[ -n "$(git -C "$PROJECT_ROOT" status --porcelain)" ]]; then
  echo "ERROR: bootstrap 前请先提交当前改动，生产基线必须对应一个 Git commit" >&2
  exit 1
fi
if ! git -C "$PROJECT_ROOT" merge-base --is-ancestor "$BOOTSTRAP_REF" HEAD; then
  echo "ERROR: BOOTSTRAP_REF 必须是当前 HEAD 或其祖先，才能随离线 bundle 上传" >&2
  exit 1
fi
LOCAL_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/hank-bootstrap.XXXXXX")"
LOCAL_BUNDLE="$LOCAL_STAGE/repository.bundle"
git -C "$PROJECT_ROOT" bundle create "$LOCAL_BUNDLE" HEAD --branches
git -C "$PROJECT_ROOT" bundle verify "$LOCAL_BUNDLE" >/dev/null 2>&1

log "上传 server Agent 基础设施到 $SSH_HOST ..."
REMOTE_STAGE="$(ssh "$SSH_HOST" 'mktemp -d /tmp/hank-bootstrap.XXXXXX')"
if [[ ! "$REMOTE_STAGE" =~ ^/tmp/hank-bootstrap\.[[:alnum:]]+$ ]]; then
  echo "ERROR: 远端临时目录异常: $REMOTE_STAGE" >&2
  exit 1
fi

scp -q \
  "$PROJECT_ROOT/deploy/hank-deploy" \
  "$PROJECT_ROOT/deploy/hank-server.service" \
  "$PROJECT_ROOT/deploy/hank-cli.service" \
  "$PROJECT_ROOT/deploy/hank-quant.service" \
  "$PROJECT_ROOT/deploy/quant-slidev.nginx" \
  "$PROJECT_ROOT/deploy/hank-docs.nginx" \
  "$SSH_HOST:$REMOTE_STAGE/"
scp -q "$LOCAL_BUNDLE" "$SSH_HOST:$REMOTE_STAGE/repository.bundle"
unlink "$LOCAL_BUNDLE"
LOCAL_BUNDLE=""
rmdir "$LOCAL_STAGE"
LOCAL_STAGE=""
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
cd /

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq \
  aria2 bubblewrap build-essential ca-certificates curl git libssl-dev nginx pkg-config \
  python3 rsync sudo util-linux xz-utils

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

# origin 只作为 SSH 应急入口的仓库地址元数据，正常 bootstrap、迭代与部署均不访问 GitHub。
(
  cd /
  runuser --user hank -- git config --global http.version HTTP/1.1
  runuser --user hank-build -- env HOME=/home/hank-build \
    git config --global http.version HTTP/1.1
)

if ! node -e 'const [a,b]=process.versions.node.split(".").map(Number); process.exit(a > 20 || (a === 20 && b >= 19) ? 0 : 1)' >/dev/null 2>&1; then
  # Ubuntu 18.04 的 glibc 2.27 无法运行 NodeSource 新包；使用 Node 官方
  # unofficial-builds 的 glibc-217 兼容构建，并校验发布方 SHA-256 清单。
  NODE_VERSION="20.19.4"
  NODE_ARCHIVE="node-v${NODE_VERSION}-linux-x64-glibc-217.tar.xz"
  NODE_URL="https://unofficial-builds.nodejs.org/download/release/v${NODE_VERSION}"
  NODE_STAGE="$(mktemp -d /tmp/hank-node.XXXXXX)"
  aria2c --allow-overwrite=true --auto-file-renaming=false \
    --console-log-level=warn --summary-interval=0 \
    --max-connection-per-server=8 --split=8 --min-split-size=1M \
    --dir="$NODE_STAGE" --out="$NODE_ARCHIVE" "$NODE_URL/$NODE_ARCHIVE"
  curl -fsSL "$NODE_URL/SHASUMS256.txt" -o "$NODE_STAGE/SHASUMS256.txt"
  (cd "$NODE_STAGE" && grep "  $NODE_ARCHIVE\$" SHASUMS256.txt | sha256sum -c -)
  install -d -o root -g root -m 755 "/opt/node-v${NODE_VERSION}"
  tar -xJf "$NODE_STAGE/$NODE_ARCHIVE" --strip-components=1 -C "/opt/node-v${NODE_VERSION}"
  ln -sfn "/opt/node-v${NODE_VERSION}/bin/node" /usr/local/bin/node
  ln -sfn "/opt/node-v${NODE_VERSION}/bin/npm" /usr/local/bin/npm
  ln -sfn "/opt/node-v${NODE_VERSION}/bin/npx" /usr/local/bin/npx
  rm -rf -- "$NODE_STAGE"
fi
npm install --global --prefix /usr/local pnpm@10.26.2

if [[ ! -x /home/hank-build/.cargo/bin/cargo ]]; then
  runuser --user hank-build -- env \
    HOME=/home/hank-build \
    RUSTUP_DIST_SERVER=https://rsproxy.cn \
    RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup \
    bash -c \
    'curl --proto "=https" --tlsv1.2 -sSf https://rsproxy.cn/rustup-init.sh -o /tmp/hank-rustup.sh && sh /tmp/hank-rustup.sh -y --default-toolchain stable --profile minimal && rm -f /tmp/hank-rustup.sh'
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
if [[ ! -x /home/hank-build/.cargo/bin/rg ]]; then
  runuser --user hank-build -- env HOME=/home/hank-build \
    PATH=/home/hank-build/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
    /home/hank-build/.cargo/bin/cargo install ripgrep --locked
fi
ln -sfn /home/hank-build/.cargo/bin/rg /usr/local/bin/rg

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
chown -R hank:hank /opt/hank/logs
install -d -o hank -g hank-workspace -m 2750 /opt/hank-src
install -d -o hank -g hank-workspace -m 2770 /opt/hank-worktrees
install -d -o hank-build -g hank-workspace -m 2770 /opt/hank-workspaces
install -d -o hank -g hank-workspace -m 2770 /opt/hank-agent-state
install -d -o root -g root -m 755 /workspace /agent-home /git-common

BUNDLE_FILE="$(mktemp /tmp/hank-bootstrap-repository.XXXXXX.bundle)"
install -o hank -g hank -m 600 "$STAGE/repository.bundle" "$BUNDLE_FILE"
cleanup_bundle() { unlink "$BUNDLE_FILE" 2>/dev/null || true; }
trap cleanup_bundle EXIT

if [[ ! -d /opt/hank-src/.git ]]; then
  runuser --user hank -- git -C /opt/hank-src init
fi
if runuser --user hank -- git -C /opt/hank-src config --get remote.origin.url >/dev/null 2>&1; then
  runuser --user hank -- git -C /opt/hank-src remote set-url origin "$REPO_URL"
else
  runuser --user hank -- git -C /opt/hank-src remote add origin "$REPO_URL"
fi
runuser --user hank -- git -C /opt/hank-src fetch "$BUNDLE_FILE" HEAD
if ! runuser --user hank -- git -C /opt/hank-src cat-file -e "$BOOTSTRAP_REF^{commit}"; then
  echo "ERROR: BOOTSTRAP_REF=$BOOTSTRAP_REF 不在本机上传的 Git bundle 中" >&2
  exit 1
fi
BASE_SHA="$(runuser --user hank -- git -C /opt/hank-src rev-parse "$BOOTSTRAP_REF^{commit}")"
if ! runuser --user hank -- git -C /opt/hank-src rev-parse --verify trace-production >/dev/null 2>&1; then
  runuser --user hank -- git -C /opt/hank-src update-ref refs/heads/trace-production "$BASE_SHA"
fi
if ! runuser --user hank -- git -C /opt/hank-src symbolic-ref --quiet HEAD >/dev/null 2>&1 || \
   ! runuser --user hank -- git -C /opt/hank-src rev-parse --verify HEAD >/dev/null 2>&1; then
  runuser --user hank -- git -C /opt/hank-src symbolic-ref HEAD refs/heads/trace-production
  runuser --user hank -- git -C /opt/hank-src reset --hard trace-production
fi
chown -R hank:hank-workspace /opt/hank-src
chgrp -R hank-workspace /opt/hank-worktrees /opt/hank-workspaces
# 构建用户只需读取生产仓库；公共 config、hooks 和 trace-production ref 不可写。
chmod -R u+rwX,g+rX,o-rwx /opt/hank-src
chmod -R u+rwX,g+rwX,o-rwx /opt/hank-worktrees /opt/hank-workspaces
find /opt/hank-src /opt/hank-worktrees /opt/hank-workspaces -type d -exec chmod g+s {} +

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
runuser --user hank -- git -C /opt/hank-src config core.sharedRepository group
# worktree 从创建到 checkout 都由 hank-build 执行。Git 2.17 不支持
# safe.directory 通配符；基线仓库在这里注册，话题 worktree 由 server 注册精确路径。
runuser --user hank-build -- env HOME=/home/hank-build \
  git config --global --unset-all safe.directory '^/opt/hank-worktrees/\*$' || true
if ! runuser --user hank-build -- env HOME=/home/hank-build \
  git config --global --get-all safe.directory | grep -Fxq /opt/hank-src; then
  runuser --user hank-build -- env HOME=/home/hank-build \
    git config --global --add safe.directory /opt/hank-src
fi

install -d -o root -g root -m 755 /usr/local/libexec
install -o root -g root -m 755 "$STAGE/hank-deploy" /usr/local/libexec/hank-deploy
cat > /etc/sudoers.d/hank-deploy <<'EOF'
hank ALL=(root) NOPASSWD: /usr/local/libexec/hank-deploy *
EOF
cat > /etc/sudoers.d/hank-agent-cli <<'EOF'
hank ALL=(hank-build) NOPASSWD: NOLOG_INPUT: NOLOG_OUTPUT: /opt/hank/current/hank-server --agent-sandbox-launcher /usr/bin/bwrap *
EOF
chmod 440 /etc/sudoers.d/hank-deploy
chmod 440 /etc/sudoers.d/hank-agent-cli
visudo -cf /etc/sudoers.d/hank-deploy
visudo -cf /etc/sudoers.d/hank-agent-cli

install -o root -g root -m 644 "$STAGE/hank-server.service" /etc/systemd/system/hank-server.service
install -o root -g root -m 644 "$STAGE/hank-cli.service" /etc/systemd/system/hank-cli.service
install -o root -g root -m 644 "$STAGE/hank-docs.nginx" /etc/nginx/sites-enabled/hank-docs
# /opt/hank-quant* 目录仍创建，供独立 quant 仓库部署复用；unit/nginx 由 quant 仓库安装

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
general_workspaces_root = "/opt/hank-workspaces"
base_ref = "trace-production"
deploy_jobs_dir = "/opt/hank/deploy-jobs"
deploy_helper = "/usr/local/libexec/hank-deploy"
execution_user = "hank-build"
agent_cli_root = "/opt/hank-agent-cli"
agent_state_root = "/opt/hank-agent-state"
agent_timeout_secs = 1800
agent_output_limit_bytes = 2097152
agent_sandbox_bin = "/usr/bin/bwrap"
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
    "general_workspaces_root": 'general_workspaces_root = "/opt/hank-workspaces"\n',
    "base_ref": 'base_ref = "trace-production"\n',
    "deploy_jobs_dir": 'deploy_jobs_dir = "/opt/hank/deploy-jobs"\n',
    "deploy_helper": 'deploy_helper = "/usr/local/libexec/hank-deploy"\n',
    "execution_user": 'execution_user = "hank-build"\n',
    "agent_cli_root": 'agent_cli_root = "/opt/hank-agent-cli"\n',
    "agent_state_root": 'agent_state_root = "/opt/hank-agent-state"\n',
    "agent_timeout_secs": "agent_timeout_secs = 1800\n",
    "agent_output_limit_bytes": "agent_output_limit_bytes = 2097152\n",
    "agent_sandbox_bin": 'agent_sandbox_bin = "/usr/bin/bwrap"\n',
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

log "bootstrap 完成。下一步运行 make deploy / deploy-cli 建立正式 release；quant 请在独立仓库 make deploy。"
