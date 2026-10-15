# PhotoCraft VOS 应用打包说明

本目录是 VOS app `com.ictrek.photocraft` 的安装包模板，形态与 VAI 里的兄弟应用一致
（`apps/image-studio/ictrek.app`、`apps/nowen-video/ictrek.app`，本目录部分脚本以
`apps/motrix-next/ictrek.app` 为蓝本）。正式发布以 `update_version.sh` 触发的 GitHub Actions
为准（`.github/workflows/vos-release.yml`）。

## 第 0 期范围

- 交付**上游 web 构建产物**（wasm + nginx），无后端、无持久化、无 system 依赖。
- 文档只活在这个浏览器标签页里（与上游 Web 版行为一致）；没有安装参数。
- 两层 profile 只用于挑选按架构区分的 nginx 基础镜像，站点内容与架构无关。

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

镜像 tag 与上面解耦（VAI 惯例）：只有一个 tag `<arch>_<YYYYMMDD>_<sha>`，`package.sh` 和
`audit-package.sh` 都从飞书读它并用同一个 `TAG_RE` 校验。版本不在 tag 里——它在
`org.opencontainers.image.version` label、`VERSION` 和 `UPSTREAM.yml` 里。（以前另打一个可读的
`<完整版本>-<arch>` 发布 tag，已删：没有任何脚本引用它，`TAG_RE` 也拒绝该格式，而且它不带
`--tag-suffix`，同一天第二次构建会覆盖第一次。）

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
