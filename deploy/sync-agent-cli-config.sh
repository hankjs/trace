#!/usr/bin/env bash
# 将本机 Claude Code / Codex 的第三方 API 配置安全同步到 wananyun。

set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
CLAUDE_SETTINGS="${CLAUDE_SETTINGS:-$HOME/.claude/settings.json}"
CODEX_AUTH="${CODEX_AUTH:-$HOME/.codex/auth.json}"
CODEX_CONFIG="${CODEX_CONFIG:-$HOME/.codex/config.toml}"
LOCAL_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/hank-agent-config.XXXXXX")"
REMOTE_STAGE=""

cleanup() {
  if [[ -n "$REMOTE_STAGE" && "$REMOTE_STAGE" == /tmp/hank-agent-config.* ]]; then
    ssh "$SSH_HOST" "find '$REMOTE_STAGE' -type f -delete 2>/dev/null; find '$REMOTE_STAGE' -depth -type d -empty -delete 2>/dev/null" || true
  fi
  if [[ "$LOCAL_STAGE" == "${TMPDIR:-/tmp}"/hank-agent-config.* ]]; then
    find "$LOCAL_STAGE" -type f -delete 2>/dev/null || true
    find "$LOCAL_STAGE" -depth -type d -empty -delete 2>/dev/null || true
  fi
}
trap cleanup EXIT

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

for file in "$CLAUDE_SETTINGS" "$CODEX_AUTH" "$CODEX_CONFIG"; do
  [[ -f "$file" ]] || {
    echo "ERROR: 本机配置不存在: $file" >&2
    exit 1
  }
done

install -m 600 "$CLAUDE_SETTINGS" "$LOCAL_STAGE/claude-settings.json"
install -m 600 "$CODEX_AUTH" "$LOCAL_STAGE/codex-auth.json"
install -m 600 "$CODEX_CONFIG" "$LOCAL_STAGE/codex-config.toml"

# 使用结构化解析器提取运行时字段。完整源配置仍会原样保存在远端受限目录中，
# 但飞书 Agent 只接收这里明确允许的 API、端点和模型字段。
python3 - "$LOCAL_STAGE" <<'PY'
import json
import pathlib
import sys

try:
    import tomllib
except ImportError as exc:
    raise SystemExit("ERROR: 本机 Python 需要 3.11+（缺少 tomllib）") from exc

stage = pathlib.Path(sys.argv[1])
with (stage / "claude-settings.json").open("rb") as handle:
    claude = json.load(handle)
with (stage / "codex-auth.json").open("rb") as handle:
    codex_auth = json.load(handle)
with (stage / "codex-config.toml").open("rb") as handle:
    codex = tomllib.load(handle)

claude_env = claude.get("env", {})
if not isinstance(claude_env, dict):
    raise SystemExit("ERROR: Claude settings.json 的 env 不是对象")

allowed_claude = (
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
)
values = {
    key: value
    for key in allowed_claude
    if isinstance((value := claude_env.get(key)), str) and value
}
if not any(key in values for key in ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN")):
    raise SystemExit("ERROR: Claude 配置中没有可用认证字段")

openai_key = codex_auth.get("OPENAI_API_KEY")
if not isinstance(openai_key, str) or not openai_key:
    raise SystemExit("ERROR: Codex auth.json 中没有 OPENAI_API_KEY")
values["OPENAI_API_KEY"] = openai_key

provider_name = codex.get("model_provider")
providers = codex.get("model_providers", {})
provider = providers.get(provider_name, {}) if isinstance(providers, dict) else {}
if not isinstance(provider, dict):
    raise SystemExit("ERROR: Codex 当前 model_provider 配置无效")
if provider.get("wire_api") != "responses":
    raise SystemExit("ERROR: 飞书 Codex 后端只支持 wire_api=responses 的第三方端点")
base_url = provider.get("base_url")
if not isinstance(base_url, str) or not base_url:
    raise SystemExit("ERROR: Codex 当前 model_provider 没有 base_url")
values["OPENAI_BASE_URL"] = base_url
model = codex.get("model")
if isinstance(model, str) and model:
    values["OPENAI_MODEL"] = model

def quote_systemd(value: str) -> str:
    if "\n" in value or "\r" in value or "\0" in value:
        raise SystemExit("ERROR: 配置值包含不允许的控制字符")
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'

lines = [
    "# 由 deploy/sync-agent-cli-config.sh 从本机 Claude Code / Codex 配置生成。",
    "# 包含凭据：仅允许 root:hank 0640，禁止提交或回传。",
]
lines.extend(f"{key}={quote_systemd(value)}" for key, value in values.items())
(stage / "agent-cli.env").write_text("\n".join(lines) + "\n", encoding="utf-8")
PY
chmod 600 "$LOCAL_STAGE/agent-cli.env"

(cd "$LOCAL_STAGE" && shasum -a 256 \
  claude-settings.json codex-auth.json codex-config.toml agent-cli.env > SHA256SUMS)

REMOTE_STAGE="$(ssh "$SSH_HOST" 'mktemp -d /tmp/hank-agent-config.XXXXXX')"
[[ "$REMOTE_STAGE" == /tmp/hank-agent-config.* ]] || {
  echo "ERROR: 远端临时目录异常: $REMOTE_STAGE" >&2
  exit 1
}

log "上传 Claude Code / Codex 配置到 $SSH_HOST ..."
scp -q "$LOCAL_STAGE/claude-settings.json" "$LOCAL_STAGE/codex-auth.json" \
  "$LOCAL_STAGE/codex-config.toml" "$LOCAL_STAGE/agent-cli.env" \
  "$LOCAL_STAGE/SHA256SUMS" "$SSH_HOST:$REMOTE_STAGE/"

log "安装配置并重启 hank-server ..."
ssh "$SSH_HOST" bash -s -- "$REMOTE_STAGE" <<'REMOTE'
set -euo pipefail
stage="$1"
[[ $EUID -eq 0 ]] || { echo "ERROR: 必须以 root 同步配置" >&2; exit 1; }
[[ "$stage" == /tmp/hank-agent-config.* ]] || { echo "ERROR: stage 越界" >&2; exit 1; }

cd "$stage"
sha256sum -c SHA256SUMS

release_id="$(date -u +%Y%m%dT%H%M%SZ)"
release="/opt/hank-agent-config/releases/$release_id"
install -d -o root -g hank -m 750 "$release"
install -o root -g hank -m 640 claude-settings.json "$release/claude-settings.json"
install -o root -g hank -m 640 codex-auth.json "$release/codex-auth.json"
install -o root -g hank -m 640 codex-config.toml "$release/codex-config.toml"
install -o root -g hank -m 640 agent-cli.env "$release/agent-cli.env"
ln -sfn "$release" /opt/hank-agent-config/current

install -d -o hank -g hank -m 700 /home/hank/.claude /home/hank/.codex
install -o hank -g hank -m 600 claude-settings.json /home/hank/.claude/settings.json
install -o hank -g hank -m 600 codex-auth.json /home/hank/.codex/auth.json
install -o hank -g hank -m 600 codex-config.toml /home/hank/.codex/config.toml

install -o root -g hank -m 640 agent-cli.env /opt/hank/agent-cli.env.new
mv -f /opt/hank/agent-cli.env.new /opt/hank/agent-cli.env

systemctl restart hank-server
systemctl is-active --quiet hank-server
runuser --user hank -- env HOME=/home/hank /usr/local/bin/codex --version
runuser --user hank -- env HOME=/home/hank /usr/local/bin/claude --version
REMOTE

log "配置同步完成；远端原始配置位于 /opt/hank-agent-config/current"
