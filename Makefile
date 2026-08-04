.PHONY: default help server-dev client-dev admin-dev deploy app

# 默认目标：仅提示可用命令，不执行任何操作
default: help

help:
	@echo "请指定要执行的目标，可用命令："
	@echo ""
	@echo "  开发:"
	@echo "    make server-dev          后端开发服务 (0.0.0.0:3000)"
	@echo "    make client-dev          客户端前端开发 (Tauri dev)"
	@echo "    make admin-dev           管理后台前端开发 (Vite)"
	@echo ""
	@echo "  构建:"
	@echo "    make app                 构建桌面客户端并安装到 /Applications"
	@echo ""
	@echo "  部署:"
	@echo "    make deploy              本地交叉编译 server + 构建 admin，推产物到线上"
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

# 部署：本地 zigbuild 交叉编译 + 本地构建 admin，只把产物推到线上
deploy:
	./deploy/deploy.sh
