.PHONY: server-dev client-dev admin-dev quant-dev quant-web-dev deploy deploy-cli deploy-quant app cli-dev cli

# 后端开发服务 (0.0.0.0:3000)
server-dev:
	cargo run -p hank-server

# 客户端前端开发 (Vite)
client-dev:
	cd client && pnpm tauri dev

# 管理后台前端开发 (Vite)
admin-dev:
	cd admin && pnpm dev

# quant 量化系统后端开发 (FastAPI, 0.0.0.0:8100, --reload)
quant-dev:
	cd quant && uv run uvicorn app.main:app --reload --port 8100

# quant 量化系统前端开发 (Vite, /api 代理到 8100)
quant-web-dev:
	cd quant/web && pnpm dev

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

# 部署 hank-cli 到线上 (服务器构建 -> 安装 -> systemd 重启, 需先跑过 make deploy)
deploy-cli:
	./deploy/deploy-cli.sh

# 部署 quant 量化系统到线上 (与 hank-server 同一台服务器, 端口 8100)
# 跳过依赖安装: make deploy-quant SKIP_DEPS=--skip-deps
deploy-quant:
	./deploy/deploy-quant.sh $(SKIP_DEPS)

# hank-cli 远程终端节点开发 (前台运行，读取 ~/.hank-cli/config.toml)
# 可用参数覆盖配置: make cli-dev SERVER=http://x:3000 USERNAME=u PASSWORD=p
cli-dev:
	cd cli && cargo run -- $(if $(SERVER),--server $(SERVER)) $(if $(USERNAME),--username $(USERNAME)) $(if $(PASSWORD),--password $(PASSWORD))

# 构建 hank-cli (release) 并安装到 /opt/homebrew/bin
cli:
	cd cli && cargo build --release
	cp cli/target/release/hank-cli /opt/homebrew/bin/
