# PhotoCraft — VOS integration entry points.
#
# The stock upstream workflow stays `cargo xtask …` (see docs/development.md). This Makefile only
# carries the VOS packaging and local-development targets, mirroring the sibling apps' Makefiles
# (../vai/apps/*/Makefile).
#
# Development inside the platform:
#   make vos-dev               nginx fronting the dev build (container, watch + auto reload) + a route
#   VOS_DEV_WEB=2 make vos-dev trunk dev server only, no container
#   VOS_DEV_WEB=0 make vos-dev fidelity mode: dist/web served by the VOS image's nginx
#   make vos-dev-down          remove the route and stop the server/container
#
# Packaging:
#   make vos-package           dry-run package (template check, no Feishu)
#   make vos-package-release   read image tags from Feishu and build the real pull package
#   make vos-scope             list the diff against the upstream base (must stay inside the whitelist)
#   make vos-release           bump the version, tag and push (CI packages and publishes)
DEFAULT_GOAL := help

.PHONY: help vos-dev vos-dev-down vos-dev-status vos-dev-logs vos-dev-web vos-dev-route-off \
        vos-package vos-package-release vos-audit vos-scope vos-release

help:
	@grep -Eh '^[a-zA-Z_.-]+:.*## ' $(MAKEFILE_LIST) | sort | \
	awk 'BEGIN {FS = ":.*## "}; {printf "\033[36m%-22s\033[0m %s\n", $$1, $$2}'

## ---- VOS local development --------------------------------------------------------------

vos-dev: ## [VOS] 联调：nginx 前置 dev 构建 + 平台临时路由（VOS_DEV_WEB=2 纯 trunk，VOS_DEV_WEB=0 保真）
	scripts/vos-dev.sh up

vos-dev-down: ## [VOS] 撤临时路由并停止 dev server / 容器
	scripts/vos-dev.sh down

vos-dev-status: ## [VOS] 查看模式、端口、临时路由与平台流量归属
	scripts/vos-dev.sh status

vos-dev-logs: ## [VOS] 跟随 nginx 容器日志
	scripts/vos-dev.sh logs

vos-dev-web: ## [VOS] 构建 dist/web（走镜像构建阶段，可选 CRAFT_FONTS_DIR 带入字体）
	scripts/vos-dev.sh build-web

vos-dev-route-off: ## [VOS] 只撤临时路由（流量切回平台上已安装的应用）
	scripts/vos-dev.sh route-off

## ---- VOS packaging ----------------------------------------------------------------------

vos-package: ## [VOS] 本地打包（--dry-run：占位镜像 tag，不读飞书，仅验证模板）
	ictrek.app/scripts/package.sh --dry-run

vos-package-release: ## [VOS] 正式打包：从飞书读镜像 tag 并生成 pull tar
	ictrek.app/scripts/package.sh

vos-audit: ## [VOS] 校验已生成的包：make vos-audit PKG=ictrek.app/dist/x_pull.tar
	@test -n "$(PKG)" || { echo "usage: make vos-audit PKG=ictrek.app/dist/photocraft_<version>_pull.tar" >&2; exit 1; }
	ictrek.app/scripts/audit-package.sh "$(PKG)"

vos-scope: ## [VOS] 列出相对上游基线的二开文件（防漂移自查）
	ictrek.app/scripts/verify-scope.sh --list

vos-release: ## [VOS] 发布：默认跟随本地 main 已合并的上游 core（未变动则递增 build）；ARGS="--same|--upstream X.Y.Z|--bump minor"
	ictrek.app/scripts/update_version.sh $(ARGS)
