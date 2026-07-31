#!/usr/bin/env bash
# 将 wananyun 的生产基线和飞书话题分支以 Git bundle 拉回本机。
# 本脚本只更新 refs/remotes/wananyun/*，不会修改工作区、合并或 push。
set -euo pipefail

SSH_HOST="${SSH_HOST:-wananyun}"
REMOTE_NAME="${REMOTE_NAME:-wananyun}"
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRODUCTION_REF="refs/remotes/$REMOTE_NAME/trace-production"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

case "$REMOTE_NAME" in
  *[!A-Za-z0-9._-]*|'')
    echo "ERROR: REMOTE_NAME 只能包含字母、数字、点、下划线和连字符" >&2
    exit 1
    ;;
esac

git -C "$PROJECT_ROOT" rev-parse --git-dir >/dev/null
LOCAL_HEAD="$(git -C "$PROJECT_ROOT" rev-parse HEAD)"
LOCAL_BRANCH="$(git -C "$PROJECT_ROOT" symbolic-ref --quiet --short HEAD || true)"
LOCAL_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/hank-sync.XXXXXX")"
LOCAL_BUNDLE="$LOCAL_STAGE/repository.bundle"
REMOTE_BUNDLE=""

cleanup() {
  if [[ "$REMOTE_BUNDLE" =~ ^/tmp/hank-sync\.[[:alnum:]]+\.bundle$ ]]; then
    ssh "$SSH_HOST" "unlink '$REMOTE_BUNDLE'" >/dev/null 2>&1 || true
  fi
  [[ ! -f "$LOCAL_BUNDLE" ]] || unlink "$LOCAL_BUNDLE"
  rmdir "$LOCAL_STAGE" 2>/dev/null || true
}
trap cleanup EXIT

log "在 $SSH_HOST 生成离线 Git bundle ..."
REMOTE_BUNDLE="$(
  ssh "$SSH_HOST" "cd / && runuser --user hank -- mktemp /tmp/hank-sync.XXXXXX.bundle"
)"
if [[ ! "$REMOTE_BUNDLE" =~ ^/tmp/hank-sync\.[[:alnum:]]+\.bundle$ ]]; then
  echo "ERROR: 远端 Git bundle 路径异常: $REMOTE_BUNDLE" >&2
  exit 1
fi

ssh "$SSH_HOST" bash -s -- "$REMOTE_BUNDLE" <<'REMOTE'
set -euo pipefail
bundle="$1"
repo="/opt/hank-src"
cd /
if [[ ! -d "$repo/.git" ]]; then
  echo "ERROR: $repo 不是 Git 工作区，请先运行 make bootstrap-server-agent" >&2
  exit 1
fi
runuser --user hank -- git -C "$repo" show-ref --verify --quiet refs/heads/trace-production || {
  echo "ERROR: wananyun 缺少 trace-production 分支" >&2
  exit 1
}
runuser --user hank -- git -C "$repo" bundle create "$bundle" --branches
runuser --user hank -- git -C "$repo" bundle verify "$bundle" >/dev/null 2>&1
REMOTE

log "下载并导入 refs/remotes/$REMOTE_NAME/* ..."
scp -q "$SSH_HOST:$REMOTE_BUNDLE" "$LOCAL_BUNDLE"
ssh "$SSH_HOST" "unlink '$REMOTE_BUNDLE'"
REMOTE_BUNDLE=""
git -C "$PROJECT_ROOT" bundle verify "$LOCAL_BUNDLE" >/dev/null 2>&1

FETCH_SPECS=()
FEISHU_REFS=()
FEISHU_COUNT=0
while read -r _sha ref; do
  case "$ref" in
    refs/heads/trace-production)
      FETCH_SPECS+=("+$ref:$PRODUCTION_REF")
      ;;
    refs/heads/feishu/*)
      local_ref="refs/remotes/$REMOTE_NAME/${ref#refs/heads/}"
      FETCH_SPECS+=("+$ref:$local_ref")
      FEISHU_REFS+=("$local_ref")
      FEISHU_COUNT=$((FEISHU_COUNT + 1))
      ;;
  esac
done < <(git -C "$PROJECT_ROOT" bundle list-heads "$LOCAL_BUNDLE")

if [[ ${#FETCH_SPECS[@]} -eq 0 ]] || [[ " ${FETCH_SPECS[*]} " != *"refs/heads/trace-production:$PRODUCTION_REF"* ]]; then
  echo "ERROR: Git bundle 中缺少 trace-production" >&2
  exit 1
fi
git -C "$PROJECT_ROOT" fetch --quiet --no-tags "$LOCAL_BUNDLE" "${FETCH_SPECS[@]}"

check_client_boundary() {
  local base_ref="$1"
  local tip_ref="$2"
  local label="$3"
  local merge_base client_paths

  merge_base="$(git -C "$PROJECT_ROOT" merge-base "$base_ref" "$tip_ref" || true)"
  if [[ -z "$merge_base" ]]; then
    echo "ERROR: $label 与本地仓库没有共同历史" >&2
    return 1
  fi
  client_paths="$(
    git -C "$PROJECT_ROOT" diff --name-only "$merge_base..$tip_ref" -- |
      awk '$0 == "client" || index($0, "client/") == 1'
  )"
  if [[ -n "$client_paths" ]]; then
    echo "ERROR: $label 包含禁止同步的 client/ 改动:" >&2
    printf '%s\n' "$client_paths" >&2
    return 1
  fi
}

check_client_boundary "$LOCAL_HEAD" "$PRODUCTION_REF" "wananyun 生产分支"
if [[ $FEISHU_COUNT -gt 0 ]]; then
  for ref in "${FEISHU_REFS[@]}"; do
    check_client_boundary "$PRODUCTION_REF" "$ref" "$ref"
  done
fi

SERVER_HEAD="$(git -C "$PROJECT_ROOT" rev-parse "$PRODUCTION_REF")"
printf '\n本机 %-18s %s\n' "${LOCAL_BRANCH:-HEAD}" "$LOCAL_HEAD"
printf '远端 %-18s %s\n' "$PRODUCTION_REF" "$SERVER_HEAD"
printf '飞书话题分支       %s 个\n' "$FEISHU_COUNT"

if [[ "$LOCAL_HEAD" == "$SERVER_HEAD" ]]; then
  echo "状态：本机与 wananyun 生产基线一致。"
elif git -C "$PROJECT_ROOT" merge-base --is-ancestor "$LOCAL_HEAD" "$SERVER_HEAD"; then
  echo "状态：wananyun 有可快进拉回的提交："
  git -C "$PROJECT_ROOT" log --oneline --no-decorate "$LOCAL_HEAD..$PRODUCTION_REF"
  printf '\n脚本未合并。检查后手动执行：\n'
  printf '  git merge --ff-only %s\n' "$PRODUCTION_REF"
  if [[ "$LOCAL_BRANCH" == "master" ]]; then
    echo "  git push origin master"
  else
    echo "  git push origin HEAD:<目标分支>"
  fi
elif git -C "$PROJECT_ROOT" merge-base --is-ancestor "$SERVER_HEAD" "$LOCAL_HEAD"; then
  echo "状态：本机领先 wananyun，当前没有需要拉回的生产提交。"
else
  echo "状态：本机与 wananyun 已分叉，拒绝自动合并。"
  echo "请先检查以下两侧提交，再手动 rebase/cherry-pick："
  git -C "$PROJECT_ROOT" log --left-right --oneline --no-decorate "$LOCAL_HEAD...$PRODUCTION_REF"
fi

pending=0
if [[ $FEISHU_COUNT -gt 0 ]]; then
  for ref in "${FEISHU_REFS[@]}"; do
    if ! git -C "$PROJECT_ROOT" merge-base --is-ancestor "$ref" "$PRODUCTION_REF"; then
      if [[ $pending -eq 0 ]]; then
        printf '\n尚未进入生产基线的话题分支：\n'
      fi
      pending=$((pending + 1))
      printf '  %s\n' "$ref"
      git -C "$PROJECT_ROOT" log -n 10 --oneline --no-decorate "$PRODUCTION_REF..$ref" |
        sed 's/^/    /'
    fi
  done
fi

log "同步完成；未修改当前分支，未连接 GitHub。"
