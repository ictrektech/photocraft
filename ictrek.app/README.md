# PhotoCraft VOS 应用打包说明

本目录是 VOS app `com.ictrek.photocraft` 的安装包模板，形态与 VAI 里的兄弟应用一致
（`apps/image-studio/ictrek.app`、`apps/nowen-video/ictrek.app`，本目录部分脚本以
`apps/motrix-next/ictrek.app` 为蓝本）。正式发布以 `update_version.sh` 触发的 GitHub Actions
为准（`.github/workflows/vos-release.yml`）。

GitHub Release 与应用商店上传使用独立任务。GitHub 先公开 Release 并校验安装包；
重跑也会解除已有草稿状态。商店任务从公开 Release 下载同一安装包，失败仅影响商店，
不会阻止或撤回 GitHub 发布。手动运行仍只验证模板，不发布版本。

## 运行方式

同一镜像包含 WebAssembly 编辑器、nginx 和一个小型 Rust 授权目录服务。
浏览器保留本机打开/下载，支持 VOS 数据访问授权目录和 AI 相册免二次登录选图。
`VOS_APP_EXPOSED_PATH` 由平台注入，挂载到 `/exposed`；文件按平台授权读写。
授权服务每次请求向 `VOS_OIDC_USERINFO_URL` 校验当前用户，不保存应用私有文件。
原相册图片保持不变，编辑副本可保存到本机或 VOS。

## 版本

```text
ictrek.app/VERSION    0.8.2-vos.1          ← 上游 core 0.8.2 + VOS 第 1 次构建
ictrek.app/UPSTREAM.yml                    ← core / ref / fork_build（供 CI 与人工核对）
```

| 场景 | 命令 | 结果 |
| --- | --- | --- |
| 跟上游（改了上游代码后重新构建） | `./update_version.sh --upstream 0.9.0` | `0.9.0-vos.1` |
| 同一 core 的第二次交付 | `./update_version.sh` | `0.8.2-vos.2` |
| 我们主动抬 core（不兼容二开） | `./update_version.sh --core 0.9.0 --base 0.8.2` | `0.9.0-vos.1+up0.8.2` |

镜像 tag 固定为 `<arch>_<YYYYMMDD>`；版本和提交记录在 OCI label 中。
构建机不执行 Git：本地取得提交后，rsync 代码，并显式传入 `PHOTOCRAFT_BUILD_SHA`。
`package.sh` 与 `audit-package.sh` 同时兼容先前含提交后缀的镜像，便于回滚。

## 打包（本地调试用）

```bash
./scripts/package.sh            # → dist/photocraft_<版本>_pull.tar
./scripts/package.sh --audit-only
./scripts/audit-package.sh dist/photocraft_0.8.2-vos.1_pull.tar
```

发布不要在本地读飞书打包：本地只用来验证模板。包是 pull 模式，外层 tar 里只有一个
`app.tar.gz`（`.env` 里是渲染后的镜像地址），**不嵌入镜像**。

## 脚本

| 脚本 | 作用 |
| --- | --- |
| `scripts/package.sh` | 读飞书镜像 tag → 渲染 `__APP_VERSION__` → 生成 pull tar → 自校验 |
| `scripts/update_version.sh` | 递增 `VERSION`、写 `UPSTREAM.yml`、提交并推 `vos-photocraft-v*` 触发 tag |
| `scripts/verify-scope.sh` | 防漂移：列出相对上游基线的二开文件，越出白名单即失败 |
| `scripts/audit-package.sh` | 对已生成的 tar 做独立校验（CI 用） |

## 相关文档

- 设计记录：`plan/vos-integration.md`（本地，gitignored）
- 仓库内文档：`docs/vos-integration.md`
- VOS 联调：仓库根的 `make vos-dev`

### 导出成品到 AI 相册

保存和导出都可下载到本机或写入 VOS。PNG、JPEG、WebP 成品还可选择“导出成品图片到 AI 相册”。
上传复用当前用户会话，核验相册中的图片记录后再提示成功；原图保留。
PSD / PCraft 工程保存到本机或 VOS。异步保存失败会明确报错并恢复未保存状态，便于重试。
