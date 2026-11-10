# PhotoCraft VOS API

## Addresses and protocols

VOS prefix: `/app/com.ictrek.photocraft`; container: `http://photocraft-web:8080`. No independent host port is published. Nginx forwards `/storage/*` to Rust on internal8081; external callers use the frontend `/storage/*` route. The sidebar API guide provides both languages and quick navigation.

The file bridge is HTTP, `/storage/mcp` is Streamable HTTP MCP, and standalone desktop `127.0.0.1:<control-port>` is line-delimited JSON over TCP. These are separate protocols.

## Identity, permissions and Keys

Each HTTP/MCP request sends `Authorization: Bearer $VOS_TOKEN`. VOS userinfo verifies the user; successful verdicts are briefly cached (default60seconds). Application-context/admin-app tokens are not person identities and are refused. A Mcp-Session-Id routes requests but does not authenticate them. No personal API Key create/update/list API exists. Scheduler/provider Keys cannot replace a VOS-user token.

Access is limited to platform-granted public directories and the user's own users/USERNAME/data. Scaffolding, another user's directories and escaping symlinks remain inaccessible. Model visibility/Keys, node registration and compute budgets belong to V-Scheduler, not this file bridge.

## Complete file-bridge fields

JSON successes guarantee code=0 and data; do not require a msg field to be present.

- GET `/storage/volumes`:200 `{code:0,data:{total,data:[{uuid,name,adapter}]}}`. The x_HASH adapter is an authorized-root ID, not an absolute path. Use the returned adapter in later requests.
- GET `/storage/index?adapter=ID&path=/RELATIVE_DIR`:200 `{code:0,data:{files:[{type,path,basename,extension,file_size,storage}]}}`. Type is dir/file, file_size is bytes and storage is the adapter. Entries are basename-sorted; dotfiles are hidden. No pagination:10000entries trigger rejection requesting a smaller directory, not silent truncation.
- GET `/storage/download?adapter=ID&path=/RELATIVE_FILE`: application/octet-stream bytes, not a JSON envelope. Default maximum256MiB; oversize413. An adapter never bypasses file/path authorization.
- POST `/storage/upload?adapter=ID&path=/RELATIVE_DIR&name=FILENAME`: raw bytes, not multipart/JSON;200 `{code:0,data:"saved"}`. Default256MiB body limits apply in nginx and Rust. A neighboring temporary file is synced then atomically replaces the target. **Existing same-name files are overwritten**; failures remove the temporary file.
- POST `/storage/new_folder`: application/json `{adapter:string,path:string,name:string}`;200 `{code:0,data:"created"}`. Creates one directory rather than recursive mkdir; name must be a valid single filename.
- POST `/storage/album/session`: application/json `{origin:"https://VOS_HOST:PORT"}` with the current user's Bearer. Origin must match Host, contain no path/query/fragment, and HTTP origins are upgraded to the platform's external HTTPS port. Success returns the native AI Album session (including accessToken). This is a sensitive user credential, not a long-lived PhotoCraft Key to print/store as one. Unavailable/invalid Album502, authentication failure401.

Do not invent rename/delete/personal-Key HTTP endpoints. Framework parameter/JSON extraction errors can be native400/422 text rather than the business JSON envelope.

```bash
BASE=http://photocraft-web:8080
curl "$BASE/storage/volumes" -H "Authorization: Bearer $VOS_TOKEN"
# Set ADAPTER from the previous response; URL-encode paths and filenames.
curl --get "$BASE/storage/index" -H "Authorization: Bearer $VOS_TOKEN" --data-urlencode "adapter=$ADAPTER" --data-urlencode 'path=/'
curl --get "$BASE/storage/download" -H "Authorization: Bearer $VOS_TOKEN" --data-urlencode "adapter=$ADAPTER" --data-urlencode 'path=/example.png' --output example.png
```

## MCP initialization, discovery and sessions

POST/GET/DELETE `/storage/mcp` use MCP JSON-RPC2.0 with JSON/SSE. Every request still carries VOS Bearer. Initialize, retain the returned Mcp-Session-Id, send notifications/initialized, then call tools/list or tools/call with the session header. An SDK handles SSE and version negotiation.

```bash
MCP=http://photocraft-web:8080/storage/mcp
curl -i "$MCP" -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"example-agent","version":"1.0"}}}'
# Save the returned session header as MCP_SESSION; use the negotiated version.
curl "$MCP" -H "Authorization: Bearer $VOS_TOKEN" -H "Mcp-Session-Id: $MCP_SESSION" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{"jsonrpc":"2.0","method":"notifications/initialized"}'
curl "$MCP" -H "Authorization: Bearer $VOS_TOKEN" -H "Mcp-Session-Id: $MCP_SESSION" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
curl "$MCP" -H "Authorization: Bearer $VOS_TOKEN" -H "Mcp-Session-Id: $MCP_SESSION" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"session_list","arguments":{}}}'
```

VOS uses per-user headless documents. The browser canvas and MCP memory are not automatically the same document. Keep the MCP session when editing and explicitly doc_save/doc_export. Defaults:8resident users,30-minute idle reclamation; all workspaces in use yields429. Unsaved memory is not durable across restart/reclamation.

Continuous in-memory edits need a session-capable protocol, such as the verified2025-03-26. Rmcp handles2026-07-28 statelessly, so document state cannot be assumed to survive calls under that version. Save before closing. After session loss, reinitialize and inspect files rather than blindly replaying mutations.

## Commands, jobs, cancellation and results

Tools/list returns25native tools and their inputSchema. Start with session_list and doc_new/doc_open. Use command_list(filter/enabled_only) to discover IDs, parameter documentation and disabled reasons, then command_run. Never derive IDs from menu text. Command_batch is limited to256steps; each command has its own history step. Failure or budget exhaustion retains completed steps/results rather than rolling back the entire batch.

Command_run defaults wait=true. With wait=false, long operations return `{job:id}`; poll jobs_list. Jobs_cancel takes an optional numeric job ID; **omitting it cancels all running jobs**. Supply the ID to stop one. Disconnecting HTTP/closing MCP is not a saved-document rollback. Timeouts/response-budget errors can occur after execution; inspect document/job state before retrying.

Tool results primarily use MCP content text (JSON text), plus optional image/base64/mimeType content. Do not require structuredContent. Inspect isError and JSON-RPC error as well as HTTP status. Bridge-mode ui_* and control_call need a live desktop bridge and are not usable UI automation in VOS headless mode.

Default MCP frame limit1MiB. Previews are bounded by2048side/5MiB PNG and responses by8MiB. Nginx's300-second read timeout is not execution admission; use background jobs for long work. Typical failures:401 identity,403 app token,404/400 unavailable path,400 invalid format/path,413 oversize,429 workspace capacity and502 identity/Album upstream. Rust business failures use `{code:10001|10007|10002,msg,data:null}` for unauthenticated/not-found/other errors; proxy/framework/MCP errors retain their own formats.

## Desktop control protocol

Standalone desktop control uses a token file/PHOTOCRAFT_CONTROL_TOKEN_FILE. Authenticate first on each TCP connection, then send JSON lines `{id,method,params}`. Replies are `{id,ok,result}` or `{id,ok:false,error}`. The desktop token is not a VOS Key. At the 60-second deadline, `timeout: not run` is retryable; `timeout: no reply in 60s` may already have executed. Inspect state before replay. See the complete [desktop protocol](../../docs/control-protocol.md) and [native Rust tools](../../crates/automation/src/server.rs).

## Native MCP inputs

These schemas come from the executable native service. All25tool names and all22parameterized tools' root fields, including serde renames, match current Rust source. The live tools/list remains authoritative. Native field descriptions, required flags, nesting, enums and limits are retained.

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
