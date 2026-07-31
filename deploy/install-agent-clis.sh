#!/usr/bin/env bash
# 本机下载并校验 Claude Code / Codex Linux x64 原生制品，再通过 SSH 离线安装。

set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
CODEX_VERSION="${CODEX_VERSION:-0.146.0}"
CLAUDE_VERSION="${CLAUDE_VERSION:-2.1.220}"
LOCAL_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/hank-agent-clis.XXXXXX")"
REMOTE_STAGE=""

cleanup() {
  if [[ -n "$REMOTE_STAGE" && "$REMOTE_STAGE" == /tmp/hank-agent-clis.* ]]; then
    ssh "$SSH_HOST" "find '$REMOTE_STAGE' -type f -delete 2>/dev/null; find '$REMOTE_STAGE' -depth -type d -empty -delete 2>/dev/null" || true
  fi
  if [[ "$LOCAL_STAGE" == "${TMPDIR:-/tmp}"/hank-agent-clis.* ]]; then
    find "$LOCAL_STAGE" -type f -delete 2>/dev/null || true
    find "$LOCAL_STAGE" -depth -type d -empty -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

fetch_npm_native() {
  local metadata_url="$1"
  local output="$2"
  local metadata="$LOCAL_STAGE/$output.json"
  local tarball shasum actual

  curl --noproxy '*' -fsSL --retry 3 "$metadata_url" -o "$metadata"
  tarball="$(node -e 'const m=require(process.argv[1]); process.stdout.write(m.dist.tarball)' "$metadata")"
  shasum="$(node -e 'const m=require(process.argv[1]); process.stdout.write(m.dist.shasum)' "$metadata")"
  [[ "$tarball" == https://registry.npmjs.org/* ]] || {
    echo "ERROR: 非 npm 官方制品地址: $tarball" >&2
    exit 1
  }
  curl --noproxy '*' -fL --retry 3 --progress-bar "$tarball" -o "$LOCAL_STAGE/$output"
  actual="$(shasum -a 1 "$LOCAL_STAGE/$output" | awk '{print $1}')"
  [[ "$actual" == "$shasum" ]] || {
    echo "ERROR: $output npm SHA-1 校验失败" >&2
    exit 1
  }
  unlink "$metadata"
}

log "下载 Codex $CODEX_VERSION Linux x64 原生制品 ..."
fetch_npm_native \
  "https://registry.npmjs.org/@openai%2fcodex/${CODEX_VERSION}-linux-x64" \
  codex.tgz

log "下载 Claude Code $CLAUDE_VERSION Linux x64 原生制品 ..."
fetch_npm_native \
  "https://registry.npmjs.org/@anthropic-ai%2fclaude-code-linux-x64/$CLAUDE_VERSION" \
  claude.tgz

(cd "$LOCAL_STAGE" && shasum -a 256 codex.tgz claude.tgz > SHA256SUMS)
cat > "$LOCAL_STAGE/versions" <<EOF
CODEX_VERSION=$CODEX_VERSION
CLAUDE_VERSION=$CLAUDE_VERSION
EOF

REMOTE_STAGE="$(ssh "$SSH_HOST" 'mktemp -d /tmp/hank-agent-clis.XXXXXX')"
[[ "$REMOTE_STAGE" == /tmp/hank-agent-clis.* ]] || {
  echo "ERROR: 远端临时目录异常: $REMOTE_STAGE" >&2
  exit 1
}
log "上传已校验制品到 $SSH_HOST ..."
scp -q "$LOCAL_STAGE/codex.tgz" "$LOCAL_STAGE/claude.tgz" \
  "$LOCAL_STAGE/SHA256SUMS" "$LOCAL_STAGE/versions" "$SSH_HOST:$REMOTE_STAGE/"

log "离线安装 CLI 与 bubblewrap 沙箱 ..."
ssh "$SSH_HOST" bash -s -- "$REMOTE_STAGE" <<'REMOTE'
set -euo pipefail
stage="$1"
[[ $EUID -eq 0 ]] || { echo "ERROR: 必须以 root 安装" >&2; exit 1; }
[[ "$stage" == /tmp/hank-agent-clis.* ]] || { echo "ERROR: stage 越界" >&2; exit 1; }

cd "$stage"
sha256sum -c SHA256SUMS
# bubblewrap 负责真正的文件视图隔离；安装失败时不能降级启动 Agent。
if ! command -v bwrap >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq bubblewrap
fi
if ! command -v rg >/dev/null 2>&1; then
  [[ -x /home/hank-build/.cargo/bin/cargo ]] || {
    echo "ERROR: ripgrep 缺失，请先运行 make bootstrap-server-agent" >&2
    exit 1
  }
  runuser --user hank-build -- env HOME=/home/hank-build \
    PATH=/home/hank-build/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
    /home/hank-build/.cargo/bin/cargo install ripgrep --locked
  ln -sfn /home/hank-build/.cargo/bin/rg /usr/local/bin/rg
fi

# shellcheck disable=SC1091
source "$stage/versions"
codex_member="$(tar -tzf codex.tgz | awk '/\/codex$/ && !found {print; found=1}')"
claude_member="$(tar -tzf claude.tgz | awk '/\/claude$/ && !found {print; found=1}')"
[[ -n "$codex_member" && -n "$claude_member" ]] || {
  echo "ERROR: 原生制品内未找到 CLI 二进制" >&2
  exit 1
}

codex_release="/opt/hank-agent-cli/codex/$CODEX_VERSION"
claude_release="/opt/hank-agent-cli/claude/$CLAUDE_VERSION"
install -d -o root -g root -m 755 "$codex_release/bin" "$claude_release/bin"
tar -xOzf codex.tgz "$codex_member" > "$codex_release/bin/codex"
tar -xOzf claude.tgz "$claude_member" > "$claude_release/bin/claude"
chown root:root "$codex_release/bin/codex" "$claude_release/bin/claude"
chmod 755 "$codex_release/bin/codex" "$claude_release/bin/claude"
ln -sfn "$codex_release" /opt/hank-agent-cli/codex/current
ln -sfn "$claude_release" /opt/hank-agent-cli/claude/current
ln -sfn /opt/hank-agent-cli/codex/current/bin/codex /usr/local/bin/codex
ln -sfn /opt/hank-agent-cli/claude/current/bin/claude /usr/local/bin/claude

install -d -o hank -g hank-workspace -m 2770 /opt/hank-agent-state
install -d -o root -g root -m 755 /workspace /agent-home /git-common
cat > /etc/sudoers.d/hank-agent-cli <<'EOF'
hank ALL=(hank-build) NOPASSWD:SETENV: /usr/bin/bwrap *
EOF
chmod 440 /etc/sudoers.d/hank-agent-cli
visudo -cf /etc/sudoers.d/hank-agent-cli
if [[ ! -f /opt/hank/agent-cli.env ]]; then
  install -o root -g hank -m 640 /dev/null /opt/hank/agent-cli.env
  cat > /opt/hank/agent-cli.env <<'EOF'
# 只在 wananyun 本地维护，不同步、不提交。至少配置使用中的后端凭据。
# OPENAI_API_KEY=...
# OPENAI_BASE_URL=...  # 可选；必须兼容 OpenAI Responses API
# OPENAI_MODEL=...
# ANTHROPIC_API_KEY=...
# ANTHROPIC_AUTH_TOKEN=...
# CLAUDE_CODE_OAUTH_TOKEN=...
EOF
fi

runuser --user hank-build -- /usr/local/bin/codex --version
runuser --user hank-build -- /usr/local/bin/claude --version
REMOTE

log "安装完成；凭据文件: /opt/hank/agent-cli.env"
