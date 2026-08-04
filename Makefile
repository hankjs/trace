.PHONY: default help server-dev client-dev admin-dev bootstrap-server-agent install-agent-clis sync-agent-cli-config sync-server-agent deploy deploy-cli app cli-dev cli

# 默认目标：仅提示可用命令，不执行任何操作
default: help

help:
	@echo "请指定要执行的目标，可用命令："
	@echo ""
	@echo "  开发:"
	@echo "    make server-dev          后端开发服务 (0.0.0.0:3000)"
	@echo "    make client-dev          客户端前端开发 (Tauri dev)"
	@echo "    make admin-dev           管理后台前端开发 (Vite)"
	@echo "    make cli-dev             hank-cli 远程终端节点开发"
	@echo ""
	@echo "  构建:"
	@echo "    make app                 构建桌面客户端并安装到 /Applications"
	@echo "    make cli                 构建 hank-cli 并安装到 /opt/homebrew/bin"
	@echo ""
	@echo "  部署:"
	@echo "    make bootstrap-server-agent 首次初始化 wananyun 飞书开发环境"
	@echo "    make install-agent-clis  本机下载校验并离线安装 Claude Code / Codex"
	@echo "    make sync-agent-cli-config 同步本机 Claude Code / Codex 第三方 API 配置"
	@echo "    make sync-server-agent  拉回 wananyun 本地 Git 分支（不合并、不 push）"
	@echo "    make deploy              部署 server + admin (可 SKIP_DEPS=--skip-deps)"
	@echo "    make deploy-cli          部署 hank-cli"
	@echo ""
	@echo "  说明: quant 已独立为仓库 https://github.com/hankjs/quant"
	@echo "        本地路径 ~/projects/hank/quant；开发/部署请到该仓库执行 make dev / make deploy"

# 后端开发服务 (0.0.0.0:3000)
server-dev:
	cargo run -p hank-server

# 客户端前端开发 (Vite)
client-dev:
	cd client && pnpm tauri dev

# 管理后台前端开发 (Vite)
admin-dev:
	cd admin && pnpm dev

# 构建桌面客户端 (Tauri release) 并安装到 /Applications
app:
	cd client && pnpm tauri build
	rm -rf /Applications/Trace.app
	cp -R client/src-tauri/target/release/bundle/macos/Trace.app /Applications/
	xattr -dr com.apple.quarantine /Applications/Trace.app 2>/dev/null || true

# 部署 server + admin 到线上 (依赖检查 -> 构建 -> 部署 -> 重启)
# 跳过依赖安装: make deploy SKIP_DEPS=--skip-deps
deploy:
	./deploy/deploy.sh $(SKIP_DEPS)

# 只执行一次：创建 hank 用户、生产 Git 基线、部署 helper 与服务权限。
bootstrap-server-agent:
	./deploy/bootstrap-server-agent.sh

install-agent-clis:
	./deploy/install-agent-clis.sh

sync-agent-cli-config:
	./deploy/sync-agent-cli-config.sh

# 拉回 wananyun 的生产基线与飞书话题分支，只更新 refs/remotes/wananyun/*。
sync-server-agent:
	./deploy/sync-server-agent.sh

# 部署 hank-cli 到线上 (服务器构建 -> 安装 -> systemd 重启, 需先跑过 make deploy)
deploy-cli:
	./deploy/deploy-cli.sh

# hank-cli 远程终端节点开发 (前台运行，读取 ~/.hank-cli/config.toml)
# 可用参数覆盖配置: make cli-dev SERVER=http://x:3000 USERNAME=u PASSWORD=p
cli-dev:
	cd cli && cargo run -- $(if $(SERVER),--server $(SERVER)) $(if $(USERNAME),--username $(USERNAME)) $(if $(PASSWORD),--password $(PASSWORD))

# 构建 hank-cli (release) 并安装到 /opt/homebrew/bin
cli:
	cd cli && cargo build --release
	cp cli/target/release/hank-cli /opt/homebrew/bin/
