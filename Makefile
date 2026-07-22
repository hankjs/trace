.PHONY: server-dev client-dev admin-dev deploy app

# 后端开发服务 (0.0.0.0:3000)
server-dev:
	cargo run -p hank-server

# 客户端前端开发 (Vite)
client-dev:
	cd client && pnpm dev

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
