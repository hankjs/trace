.PHONY: server-dev client-dev admin-dev deploy

# 后端开发服务 (0.0.0.0:3000)
server-dev:
	cargo run -p hank-server

# 客户端前端开发 (Vite)
client-dev:
	cd client && pnpm dev

# 管理后台前端开发 (Vite)
admin-dev:
	cd admin && pnpm dev

# 部署 server + admin 到线上 (依赖检查 -> 构建 -> 部署 -> 重启)
# 跳过依赖安装: make deploy SKIP_DEPS=--skip-deps
deploy:
	./deploy/deploy.sh $(SKIP_DEPS)
