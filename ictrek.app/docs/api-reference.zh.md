# PhotoCraft VOS API

## 地址和协议

VOS 同源前缀 `/app/com.ictrek.photocraft`，容器 `http://photocraft-web:8080`；没有独立宿主机映射端口。nginx 将 `/storage/*` 转到同容器 Rust 服务8081；外部使用前缀后的 `/storage/*`，不要直接连接内部8081。侧栏 API 文档提供本文的中英文版本和跳转目录。

VOS 文件桥是 HTTP；`/storage/mcp` 是 Streamable HTTP MCP；独立桌面 `127.0.0.1:<control-port>` 是逐行 JSON TCP 控制协议。三种接口不能混用。

## 身份、权限和 Key

VOS HTTP/MCP 每个请求都要带 `Authorization: Bearer $VOS_TOKEN`，由 VOS userinfo 核验用户，成功结果短期缓存（默认60秒）。应用上下文/admin app token 不是人的身份，会被拒绝；MCP Session ID 只做路由，不能替代身份。没有应用个人 API Key 的创建/修改/查询 API，V-Scheduler Key 或厂商 Key 不能代替 VOS 用户 token。

用户可访问平台挂载的 public 授权目录和自己的 `users/USERNAME/data`，不能穿越到其他用户、脚手架目录或越界软链接。模型 public/private、模型 Key、节点注册和计算预算属于 V-Scheduler 的 API，不是 PhotoCraft 文件桥的配置项。

## 文件桥完整字段

成功 JSON 为 `{code:0,data:...}`；不要要求 msg 字段必须存在。

- `GET /storage/volumes`：200 `{code:0,data:{total,data:[{uuid,name,adapter}]}}`。adapter 形如 x_HASH，是实际授权目录ID，不是绝对路径；后续请求使用这里返回的值。
- `GET /storage/index?adapter=ID&path=/RELATIVE_DIR`：200 `{code:0,data:{files:[{type,path,basename,extension,file_size,storage}]}}`。type=dir/file，file_size 为字节，storage 为 adapter。按 basename 排序、隐藏点文件；没有分页，达到10000项时拒绝并要求更小目录，不返回无标记的截断列表。
- `GET /storage/download?adapter=ID&path=/RELATIVE_FILE`：返回 application/octet-stream 原始字节，不是 JSON 包装；默认最大256MiB，超限413。不存在/只读或无权限的文件不能因为拿到 adapter 就绕过校验。
- `POST /storage/upload?adapter=ID&path=/RELATIVE_DIR&name=FILENAME`：请求体为原始字节，不是 multipart/JSON；200 `{code:0,data:"saved"}`。默认请求体256MiB（nginx和Rust服务同时约束），在目标目录写临时文件、sync后原子替换同名目标，**会覆盖已有同名文件**，失败清理临时文件。先确认目标目录及名称。
- `POST /storage/new_folder`：application/json `{adapter:string,path:string,name:string}`，200 `{code:0,data:"created"}`。创建单个目录，不是递归 mkdir；name 必须是单个合法文件名。
- `POST /storage/album/session`：application/json `{origin:"https://VOS_HOST:PORT"}`；要求当前 VOS 用户 Bearer。origin 必须同Host、不能有路径/查询/片段；HTTP origin 被转换到平台外部HTTPS端口。成功返回 AI 相册的原生会话对象（含accessToken），属于当前用户的敏感凭证，不能当作长期PhotoCraft Key保存或打印。相册不可达/返回无效会话502、身份失败401。

以上没有列出的 rename/delete/API Key 管理 HTTP 接口不应自行猜测。框架的参数/JSON抽取错误可能返回原生400/422文本，不能强制所有响应都解析同一JSON。

```bash
BASE=http://photocraft-web:8080
curl "$BASE/storage/volumes" -H "Authorization: Bearer $VOS_TOKEN"
# 将上一步的 adapter 设为 ADAPTER；目录和文件名使用URL编码
curl --get "$BASE/storage/index" -H "Authorization: Bearer $VOS_TOKEN" --data-urlencode "adapter=$ADAPTER" --data-urlencode 'path=/'
curl --get "$BASE/storage/download" -H "Authorization: Bearer $VOS_TOKEN" --data-urlencode "adapter=$ADAPTER" --data-urlencode 'path=/example.png' --output example.png
```

## MCP 初始化、发现和会话

`POST/GET/DELETE /storage/mcp` 使用 MCP JSON-RPC 2.0，接受 JSON/SSE。每次请求都带 VOS Bearer。先 initialize，保存服务返回的 Mcp-Session-Id，再发 notifications/initialized；后续 tools/list 和 tools/call 保留会话头。SDK 负责 SSE 和协议版本协商。

```bash
MCP=http://photocraft-web:8080/storage/mcp
curl -i "$MCP" -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"example-agent","version":"1.0"}}}'
# 将响应头保存为 MCP_SESSION，并使用服务协商的版本。
curl "$MCP" -H "Authorization: Bearer $VOS_TOKEN" -H "Mcp-Session-Id: $MCP_SESSION" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{"jsonrpc":"2.0","method":"notifications/initialized"}'
curl "$MCP" -H "Authorization: Bearer $VOS_TOKEN" -H "Mcp-Session-Id: $MCP_SESSION" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
curl "$MCP" -H "Authorization: Bearer $VOS_TOKEN" -H "Mcp-Session-Id: $MCP_SESSION" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"session_list","arguments":{}}}'
```

VOS 使用按用户隔离的 headless 文档；浏览器画布与MCP内存文档不是自动同步的同一文档。创建/打开后继续用同一个MCP会话编辑，显式 doc_save/doc_export 保存结果。默认最多8个驻留用户工作区、空闲30分钟回收；全部被占用时429。不要依赖重启/回收后尚未保存的文档。

有状态连续编辑应协商支持session的版本，例如已验证的2025-03-26；rmcp在2026-07-28协议下无状态处理，跨调用内存文档不能被假定保留。关闭会话前保存；未知/过期会话重新初始化后先检查文件，不能盲目重放修改。

## 命令、任务、取消和结果

tools/list 返回25项原生工具及完整 inputSchema；下方列出字段。先 session_list，再 doc_new/doc_open，用 command_list 的 filter/enabled_only 发现命令ID、params说明和禁用原因，再 command_run。不要根据界面文字猜命令ID。command_batch 最多256步，每条命令独立历史步骤，失败/预算到达时保留已执行步骤和结果，不承诺整批回滚。

command_run 的 wait 默认true；wait=false 对长命令返回 `{job:id}`，用 jobs_list 查询。jobs_cancel 的 job 为可选数值ID，**省略会取消所有运行任务**，只停一个时必须带ID。断开HTTP/关闭MCP会话不是已保存文档回滚，超时或响应预算错误可能发生在操作已经执行之后；检查文档与任务状态后再决定重试。

工具结果主要在 MCP content 的text（JSON文本）中，图片为image/base64/mimeType；不要假定一定有 structuredContent。同时检查 isError 和JSON-RPC error，HTTP200不表示工具成功。标为 Bridge mode 的 ui_* / control_call 需要连接真实桌面桥，在VOS headless模式不可当作可用UI自动化。

MCP帧默认1MiB；预览受2048边长和5MiB PNG预算、响应8MiB预算约束。通过 `/storage/mcp` 的 nginx 超时300秒不是任务执行许可；如需更长工作，使用原生后台job。身份错误401/应用token403、不可用目录404或400、格式/路径400、过大413、MCP工作区忙429、身份或相册上游故障502。Rust业务错误为 `{code:10001|10007|10002,msg,data:null}`，分别覆盖未认证、未找到和其他错误；代理/框架/MCP有自己的错误格式。

## 桌面控制协议

独立桌面用 token文件或 PHOTOCRAFT_CONTROL_TOKEN_FILE 配置TCP控制token，首次连接必须先auth；后续逐行 `{id,method,params}`，响应 `{id,ok,result}` 或 `{id,ok:false,error}`。这是桌面控制凭证，不是VOS用户Key。默认控制调用60秒：`timeout: not run`可重试，`timeout: no reply in 60s`可能已执行；确认文档后再重放。完整 [桌面控制协议](../../docs/control-protocol.md) 与 [Rust自动化工具](../../crates/automation/src/server.rs) 保持原生所有功能。

## 原生 MCP 工具输入

以下 schema 来自可执行原生服务，并已对照当前Rust源码核对25个工具名和全部22个参数工具的顶层字段（含serde重命名）。实时tools/list仍是当前会话的最终依据。字段说明保留原生英文术语，嵌套结构、required、枚举和限制均保留。

### command_batch

Run several engine commands in one call, in order (fewer round trips). Returns {completed, failed, results:[{ok, result|error}]}; stops at the first error unless stop_on_error is false. Not an atomic transaction: earlier successful steps stay applied (no rollback).

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "steps": {
        "description": "Commands to run in order: `[{\"id\": \"layer.new.layer\", \"params\": {\"name\": \"Ink\"}}, …]`. A\nstep with `\"wait\": false` starts a long command as a background job (its result is\n`{\"job\": id}`, as in `command_run`); later steps that edit the same document fail until\nthe job ends.",
        "items": {
          "additionalProperties": false,
          "properties": {
            "id": {
              "description": "Command id, e.g. `layer.new.layer`, `filter.blur.gaussianBlur`.",
              "type": "string"
            },
            "params": {
              "default": null,
              "description": "Command parameters as a JSON object (see the `params` doc in `command_list`)."
            },
            "wait": {
              "default": null,
              "description": "Wait for a long command (filters, Content-Aware Fill, Photomerge, …) to finish (default\ntrue). With false it runs as a background job and the result is `{\"job\": id}`: poll it\nwith `jobs_list`, stop it with `jobs_cancel`.",
              "type": [
                "boolean",
                "null"
              ]
            }
          },
          "required": [
            "id"
          ],
          "type": "object"
        },
        "type": "array"
      },
      "stop_on_error": {
        "default": null,
        "description": "Stop at the first failing step (default true).",
        "type": [
          "boolean",
          "null"
        ]
      }
    },
    "required": [
      "steps"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Command batch",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### command_list

List engine commands. Returns a JSON array (no wrapper object) of {id, label, menu: [path], shortcut: string|null, params: doc string, enabled: bool, disabledReason: string|null, needsDocument: bool}. `disabledReason` says why a command is off (null when enabled); `needsDocument` is true when it is off because no document is open: create or open one first (doc_new, doc_open).

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "enabled_only": {
        "default": null,
        "description": "Only commands that can run right now.",
        "type": [
          "boolean",
          "null"
        ]
      },
      "filter": {
        "default": null,
        "description": "Only commands whose id, label or keywords contain this text (case-insensitive).",
        "type": [
          "string",
          "null"
        ]
      }
    },
    "type": "object"
  },
  "annotations": {
    "title": "Command list",
    "readOnlyHint": true,
    "destructiveHint": false,
    "idempotentHint": true,
    "openWorldHint": false
  }
}
```

### command_run

Run an engine command by id with JSON params (see command_list). Returns the command's JSON result. Long commands (filters, Content-Aware Fill/Scale, Photomerge, brush import) wait to finish unless `wait` is false: then they run as a background job and the result is {job: id} (see jobs_list, jobs_cancel).

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "id": {
        "description": "Command id, e.g. `layer.new.layer`, `filter.blur.gaussianBlur`.",
        "type": "string"
      },
      "params": {
        "default": null,
        "description": "Command parameters as a JSON object (see the `params` doc in `command_list`)."
      },
      "wait": {
        "default": null,
        "description": "Wait for a long command (filters, Content-Aware Fill, Photomerge, …) to finish (default\ntrue). With false it runs as a background job and the result is `{\"job\": id}`: poll it\nwith `jobs_list`, stop it with `jobs_cancel`.",
        "type": [
          "boolean",
          "null"
        ]
      }
    },
    "required": [
      "id"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Command run",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### control_call

Bridge mode: call any control-protocol method (docs/control-protocol.md) with raw params.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "method": {
        "description": "Any control-protocol method, e.g. `ui.dialog.open`.",
        "type": "string"
      },
      "params": {
        "default": null
      }
    },
    "required": [
      "method"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Control call",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### doc_close

Close a document (default: the active one) without saving.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "index": {
        "default": null,
        "description": "Document index from `session_list` (default: the active document).",
        "format": "uint",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      }
    },
    "type": "object"
  },
  "annotations": {
    "title": "Doc close",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### doc_export

Export the document to another format (same as doc_save with an explicit path/format).

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "dpxBits": {
        "default": null,
        "description": "DPX: 8, 10, 12 or 16 bits per sample. Unset: 8 for an 8-bit document, else 16.",
        "format": "uint8",
        "maximum": 255,
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "format": {
        "default": null,
        "description": "Format override as an extension (pcraft, psd, png, jpg, tif, webp, exr, …).",
        "type": [
          "string",
          "null"
        ]
      },
      "index": {
        "default": null,
        "format": "uint",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "path": {
        "default": null,
        "description": "Target beneath the configured automation write root: relative (forward slashes) or absolute.\nThe extension selects the format. Omit to write back to the document's own file, which\nworks only for a PSD, PSB or .pcraft file kept in its own format; any other save needs\n`path`, so a flattened or converted copy never replaces the opened file.",
        "type": [
          "string",
          "null"
        ]
      },
      "quality": {
        "default": null,
        "description": "JPEG or WebP quality 1..100. A WebP saved with a quality is lossy; without one it is lossless.",
        "format": "uint8",
        "maximum": 255,
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "tgaBits": {
        "default": null,
        "description": "TGA: 24 or 32 bits per pixel. 32 writes the alpha channel (or else the transparency) as the\nalpha, 24 none. Unset: 32 with an alpha channel or transparency, else 24.",
        "format": "uint8",
        "maximum": 255,
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "tiffLayers": {
        "default": false,
        "description": "TIFF: keep the layers (Photoshop layer data). Off by default: a flat TIFF.",
        "type": "boolean"
      }
    },
    "type": "object"
  },
  "annotations": {
    "title": "Doc export",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### doc_export_data_sets

Export every data set of the document as its own flattened file beneath the write root (the capability-scoped Export Data Sets). The batch is planned first, so colliding filenames are refused before any file is written.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "data_sets": {
        "default": null,
        "description": "Only these data sets, by name; default every one of them.",
        "items": {
          "type": "string"
        },
        "type": [
          "array",
          "null"
        ]
      },
      "dir": {
        "description": "Directory beneath the configured automation write root: relative (forward slashes) or absolute.",
        "type": "string"
      },
      "format": {
        "default": null,
        "description": "Export format (default png).",
        "type": [
          "string",
          "null"
        ]
      },
      "index": {
        "default": null,
        "description": "Document index from `session_list` (default: the active document).",
        "format": "uint",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "naming": {
        "default": null,
        "description": "Filename template with `{name}`, `{index}` and `{document}` (default `{name}`).",
        "type": [
          "string",
          "null"
        ]
      }
    },
    "required": [
      "dir"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Doc export data sets",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### doc_export_layers

Export each exported layer of the document as its own file beneath the write root (the capability-scoped Layers to Files). A file that already exists is an error, not an overwrite.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "dir": {
        "description": "Directory beneath the configured automation write root: relative (forward slashes) or absolute.",
        "type": "string"
      },
      "format": {
        "default": null,
        "description": "Export format (default png).",
        "type": [
          "string",
          "null"
        ]
      },
      "index": {
        "default": null,
        "description": "Document index from `session_list` (default: the active document).",
        "format": "uint",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "prefix": {
        "default": null,
        "description": "Filename prefix (default: the document name).",
        "type": [
          "string",
          "null"
        ]
      },
      "quality": {
        "default": null,
        "description": "Encoder quality, 0..12, for the formats that take one.",
        "format": "double",
        "type": [
          "number",
          "null"
        ]
      },
      "visible_only": {
        "default": null,
        "description": "Export only the layers that are visible (default true).",
        "type": [
          "boolean",
          "null"
        ]
      }
    },
    "required": [
      "dir"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Doc export layers",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### doc_import_data_sets

Import a data set CSV from the read root into the document (the capability-scoped Import Variable Data Sets).

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "delimiter": {
        "default": null,
        "description": "Field delimiter (default `,`).",
        "type": [
          "string",
          "null"
        ]
      },
      "index": {
        "default": null,
        "description": "Document index from `session_list` (default: the active document).",
        "format": "uint",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "path": {
        "description": "Path beneath the configured automation read root: relative (forward slashes) or absolute.",
        "type": "string"
      }
    },
    "required": [
      "path"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Doc import data sets",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### doc_inspect

Document state as JSON: layer tree (top to bottom), history, selection, active layer.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "index": {
        "default": null,
        "description": "Document index from `session_list` (default: the active document).",
        "format": "uint",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      }
    },
    "type": "object"
  },
  "annotations": {
    "title": "Doc inspect",
    "readOnlyHint": true,
    "destructiveHint": false,
    "idempotentHint": true,
    "openWorldHint": false
  }
}
```

### doc_new

Create a new document (defaults: 1920x1080 RGB 8-bit white).

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "background": {
        "default": null,
        "description": "white | black | transparent | #rrggbb",
        "type": [
          "string",
          "null"
        ]
      },
      "depth": {
        "default": null,
        "description": "8 | 16 | 32",
        "format": "uint32",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "height": {
        "default": null,
        "format": "uint32",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "mode": {
        "default": null,
        "description": "rgb | gray | cmyk | lab",
        "type": [
          "string",
          "null"
        ]
      },
      "name": {
        "default": null,
        "type": [
          "string",
          "null"
        ]
      },
      "width": {
        "default": null,
        "format": "uint32",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      }
    },
    "type": "object"
  },
  "annotations": {
    "title": "Doc new",
    "readOnlyHint": false,
    "destructiveHint": false,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### doc_open

Open an image or document file and make it active. Returns index, size and import warnings.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "path": {
        "description": "Path beneath the configured automation read root: relative (forward slashes) or absolute.",
        "type": "string"
      }
    },
    "required": [
      "path"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Doc open",
    "readOnlyHint": false,
    "destructiveHint": false,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### doc_place

Place a file into the document as a Smart Object layer (Place Embedded), reading `path` beneath the read root. `scale` (percent), `fit` and `center` are the command's own.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "center": {
        "default": null,
        "description": "Centre of the placed layer, in document pixels.",
        "items": {
          "format": "double",
          "type": "number"
        },
        "maxItems": 2,
        "minItems": 2,
        "type": [
          "array",
          "null"
        ]
      },
      "fit": {
        "default": null,
        "description": "Place at a centred size instead of a scaled one.",
        "type": [
          "boolean",
          "null"
        ]
      },
      "index": {
        "default": null,
        "description": "Document index from `session_list` (default: the active document).",
        "format": "uint",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "path": {
        "description": "Path beneath the configured automation read root: relative (forward slashes) or absolute.",
        "type": "string"
      },
      "scale": {
        "default": null,
        "description": "Scale as a percentage; defaults to fitting a file larger than the canvas.",
        "format": "double",
        "type": [
          "number",
          "null"
        ]
      }
    },
    "required": [
      "path"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Doc place",
    "readOnlyHint": false,
    "destructiveHint": false,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### doc_render_preview

Render the flattened document and return it as a PNG image (bridge mode: window screenshot).

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "index": {
        "default": null,
        "description": "Headless mode only: document index (default: the active document).\nBridge previews capture the app window and reject `index`.",
        "format": "uint",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "max_side": {
        "default": null,
        "description": "Longest side of a headless preview (default 1024, maximum 2048).\nZero requests full size within that ceiling. Bridge mode returns the window screenshot.",
        "format": "uint32",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      }
    },
    "type": "object"
  },
  "annotations": {
    "title": "Doc render preview",
    "readOnlyHint": true,
    "destructiveHint": false,
    "idempotentHint": true,
    "openWorldHint": false
  }
}
```

### doc_save

Save the document. `.pcraft` is the lossless native format (incremental); other extensions (psd, png, jpg, tif, webp, exr, …) export. Without `path` only a PSD, PSB or .pcraft document is written back to its own         file. Returns warnings about anything the format cannot hold.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "dpxBits": {
        "default": null,
        "description": "DPX: 8, 10, 12 or 16 bits per sample. Unset: 8 for an 8-bit document, else 16.",
        "format": "uint8",
        "maximum": 255,
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "format": {
        "default": null,
        "description": "Format override as an extension (pcraft, psd, png, jpg, tif, webp, exr, …).",
        "type": [
          "string",
          "null"
        ]
      },
      "index": {
        "default": null,
        "format": "uint",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "path": {
        "default": null,
        "description": "Target beneath the configured automation write root: relative (forward slashes) or absolute.\nThe extension selects the format. Omit to write back to the document's own file, which\nworks only for a PSD, PSB or .pcraft file kept in its own format; any other save needs\n`path`, so a flattened or converted copy never replaces the opened file.",
        "type": [
          "string",
          "null"
        ]
      },
      "quality": {
        "default": null,
        "description": "JPEG or WebP quality 1..100. A WebP saved with a quality is lossy; without one it is lossless.",
        "format": "uint8",
        "maximum": 255,
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "tgaBits": {
        "default": null,
        "description": "TGA: 24 or 32 bits per pixel. 32 writes the alpha channel (or else the transparency) as the\nalpha, 24 none. Unset: 32 with an alpha channel or transparency, else 24.",
        "format": "uint8",
        "maximum": 255,
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "tiffLayers": {
        "default": false,
        "description": "TIFF: keep the layers (Photoshop layer data). Off by default: a flat TIFF.",
        "type": "boolean"
      }
    },
    "type": "object"
  },
  "annotations": {
    "title": "Doc save",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### doc_select

Make the document at `index` active.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "index": {
        "format": "uint",
        "minimum": 0,
        "type": "integer"
      }
    },
    "required": [
      "index"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Doc select",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### jobs_cancel

Cancel a background job by id (or every running job when `job` is omitted). The document is left unchanged.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "job": {
        "default": null,
        "description": "The job id from `command_run` / `jobs_list` (omit to cancel every running job).",
        "format": "uint64",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      }
    },
    "type": "object"
  },
  "annotations": {
    "title": "Jobs cancel",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### jobs_list

List background jobs: running ones with progress (0-1), message and elapsed time, then the last few that ended (state done|failed|cancelled with their result or error). Finished jobs are applied first.

```json
{
  "inputSchema": {
    "additionalProperties": false,
    "properties": {},
    "type": "object"
  },
  "annotations": {
    "title": "Jobs list",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### render_preview

Render a bounded flattened PNG preview (bridge mode: app window screenshot).

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "index": {
        "default": null,
        "description": "Headless mode only: document index (default: the active document).\nBridge previews capture the app window and reject `index`.",
        "format": "uint",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      },
      "max_side": {
        "default": null,
        "description": "Longest side of a headless preview (default 1024, maximum 2048).\nZero requests full size within that ceiling. Bridge mode returns the window screenshot.",
        "format": "uint32",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      }
    },
    "type": "object"
  },
  "annotations": {
    "title": "Render preview",
    "readOnlyHint": true,
    "destructiveHint": false,
    "idempotentHint": true,
    "openWorldHint": false
  }
}
```

### session_list

List open documents (index, name, size, dirty) and the active index.

```json
{
  "inputSchema": {
    "additionalProperties": false,
    "properties": {},
    "type": "object"
  },
  "annotations": {
    "title": "Session list",
    "readOnlyHint": true,
    "destructiveHint": false,
    "idempotentHint": true,
    "openWorldHint": false
  }
}
```

### ui_inspect

Bridge mode: full UI state of the live app (tool, panels, views, dialogs, windows). For the menu tree, call `control_call` with method `ui.menu.list`.

```json
{
  "inputSchema": {
    "additionalProperties": false,
    "properties": {},
    "type": "object"
  },
  "annotations": {
    "title": "Ui inspect",
    "readOnlyHint": true,
    "destructiveHint": false,
    "idempotentHint": true,
    "openWorldHint": false
  }
}
```

### ui_menu_invoke

Bridge mode: activate a menu item by id.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "id": {
        "description": "Menu item / command id.",
        "type": "string"
      }
    },
    "required": [
      "id"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Ui menu invoke",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### ui_pointer

Bridge mode: send pointer events (document coordinates) to the active tool; with a Color Picker open they sample the image into it instead.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "button": {
        "default": null,
        "description": "Mouse button: `left` (default), or `right` / `secondary`: with the Move tool or\n`{\"command\":true}` it opens the canvas layer menu, with a painting tool the Brush Preset\npicker.",
        "type": [
          "string",
          "null"
        ]
      },
      "events": {
        "description": "Events in document coordinates: `[{\"kind\":\"down|move|up\",\"x\":..,\"y\":..,\"pressure\":..}]`.",
        "items": {
          "description": "One `ui_pointer` event, for the tool schema only: events are forwarded to `ui.pointer` as JSON.",
          "properties": {
            "kind": {
              "anyOf": [
                {
                  "enum": [
                    "down",
                    "move",
                    "up"
                  ],
                  "type": "string"
                },
                {
                  "type": "null"
                }
              ],
              "description": "`down`, `move` (default) or `up`."
            },
            "pressure": {
              "description": "Pen pressure 0..1 (default 1).",
              "format": "double",
              "type": [
                "number",
                "null"
              ]
            },
            "rotation": {
              "description": "A simulated pen's barrel rotation.",
              "format": "double",
              "type": [
                "number",
                "null"
              ]
            },
            "tiltX": {
              "description": "A simulated pen's tilt, as a stylus reports it.",
              "format": "double",
              "type": [
                "number",
                "null"
              ]
            },
            "tiltY": {
              "format": "double",
              "type": [
                "number",
                "null"
              ]
            },
            "x": {
              "description": "Document x in pixels.",
              "format": "double",
              "type": "number"
            },
            "y": {
              "description": "Document y in pixels.",
              "format": "double",
              "type": "number"
            }
          },
          "required": [
            "x",
            "y"
          ],
          "type": "object"
        },
        "type": "array"
      },
      "modifiers": {
        "default": null,
        "description": "Modifier keys, e.g. `{\"shift\":true}` (also `alt`, `command`, `ctrl`, `space`)."
      }
    },
    "required": [
      "events"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Ui pointer",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```

### ui_screenshot

Bridge mode: screenshot of the live app window as PNG.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "max_side": {
        "default": null,
        "description": "Downscale the window screenshot to this longest side (maximum 2048).\nOmit or pass zero to keep the original size.",
        "format": "uint32",
        "minimum": 0,
        "type": [
          "integer",
          "null"
        ]
      }
    },
    "type": "object"
  },
  "annotations": {
    "title": "Ui screenshot",
    "readOnlyHint": true,
    "destructiveHint": false,
    "idempotentHint": true,
    "openWorldHint": false
  }
}
```

### ui_set

Bridge mode: change UI state (tool, panels, dock, zoom, center, fit, theme, brush settings; see `fields`). Unknown fields are an error.

```json
{
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "additionalProperties": false,
    "properties": {
      "fields": {
        "description": "Fields for the control method `ui.set`: tool, panels, dock, dockTabs, dockWidth, colorPanel\n({background: bool} picks which swatch the Color panel edits), maskTarget,\nvectorMaskTarget, selectionMode, zoom, center, rotation (view angle in degrees), fit, theme (pro, proMedium, studio,\nstudioLight, classic, solarizedDark, adwaita, adwaitaDark, breezeLight, breezeDark, softLight, softDark), brushSection, brushTab,\nbrushesView, brushPicker ([x, y] opens the Brush Preset picker there, null closes it),\nbrushPickerName, brushPickerStroke, brushPickerTip (the picker card's parts, at least one\non), brushPickerScale (the picker footer's card-scale slider, 0.15 to 2, 1 the standard),\nbrushSize, gradientBlendMode\n(a blend mode name, for the Gradient tool), gradientClassic (bool), eyedropperSampleSize\n(\"point\" or 1, 3, 5, 11, 31, 51, 101), eyedropperSample (current, currentAndBelow, all,\nallNoAdjustments, currentAndBelowNoAdjustments), eyedropperRing (bool), cropOverlay (thirds,\ngrid, diagonal, triangle, goldenRatio, goldenSpiral), cropOverlayShow (auto, always, never),\ncropOverlayOrientation (0..3), cropShield (an object patching show_cropped_area, enabled,\ncolor (\"matchCanvas\" or \"custom\"), custom_color ([r, g, b]), opacity (0..100),\nauto_adjust, classic_mode and auto_center_preview), cropFill (background or contentAware,\nthe Crop tool's Fill), shapeStroke (an object patching width, align, cap, join, miterLimit,\ndashes, dashOffset and opacity; null disables the stroke). Shape stroke fields use\n`shape.edit` names and units; colour comes from `tools.setColors`. camTransformOnDrop (bool,\nthe Content-Aware Move tool's Transform On Drop). tileGrid (bool, a developer overlay of the\nactive layer's storage tiles). layerFilter (an object patching by (\"kind\", \"name\" or\n\"mode\"), kinds (a list of layer kinds), name (matches part of the layer name) and mode (a\nblend mode name) for the Layers panel filter). strokeTrace (true, false, \"stderr\" or \"off\":\nthe stroke latency trace at runtime; a CSV file is set at launch with --stroke-trace, not\nover this channel). Other fields are an error."
      }
    },
    "required": [
      "fields"
    ],
    "type": "object"
  },
  "annotations": {
    "title": "Ui set",
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": false
  }
}
```
