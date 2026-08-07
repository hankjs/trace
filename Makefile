.PHONY: default help server-dev client-dev client-prod admin-dev app-dev deploy app

# 默认目标：仅提示可用命令，不执行任何操作
default: help

help:
	@echo "请指定要执行的目标，可用命令："
	@echo ""
	@echo "  开发:"
	@echo "    make server-dev          后端开发服务 (0.0.0.0:3000)"
	@echo "    make client-dev          客户端前端开发 (Tauri dev，连本地 server)"
	@echo "    make client-prod         客户端前端开发 (Tauri dev，连线上 server)"
	@echo "    make admin-dev           管理后台前端开发 (Vite)"
	@echo "    make app-dev            远程终端 App 前端 (Vite :18791)"
	@echo ""
	@echo "  构建:"
	@echo "    make app                 构建桌面客户端并安装到 /Applications"
	@echo ""
	@echo "  部署:"
	@echo "    make deploy              本地交叉编译 server + 构建 admin/app，推产物到线上"
	@echo ""
	@echo "  说明: quant 已独立为仓库 https://github.com/hankjs/quant"
	@echo "        本地路径 ~/projects/hank/quant；开发/部署请到该仓库执行 make dev / make deploy"

# 后端开发服务 (0.0.0.0:3000)
server-dev:
	cargo run -p hank-server

# 客户端前端开发 (Vite)
client-dev:
	cd client && pnpm tauri dev

# 客户端前端开发，但 API 直连线上 server（开发前端，用生产数据）
client-prod:
	cd client && VITE_API_BASE=http://111.170.174.167:3000 pnpm tauri dev

# 管理后台前端开发 (Vite)
admin-dev:
	cd admin && pnpm dev

# 远程终端 App 前端 (Vite，默认代理线上 API；本地: HANK_API=http://localhost:3000 make app-dev)
app-dev:
	cd app && pnpm dev

# 构建桌面客户端 (Tauri release) 并安装到 /Applications
app:
	cd client && pnpm tauri build
	rm -rf /Applications/Trace.app
	cp -R client/src-tauri/target/release/bundle/macos/Trace.app /Applications/
	xattr -dr com.apple.quarantine /Applications/Trace.app 2>/dev/null || true

# 部署：本地 zigbuild 交叉编译 + 本地构建 admin，只把产物推到线上
deploy:
	./deploy/deploy.sh
