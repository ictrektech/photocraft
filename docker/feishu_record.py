#!/usr/bin/env python3
"""Record a published image tag in the VAI release spreadsheet.

The spreadsheet, its layout (row 1 = component, row 2 = repository, row 4+ = one row per build
date, newest first) and the credentials are the ones every VAI app uses; see the VAI AGENTS.md
"Feishu Release Spreadsheet" section. This script is a small, dependency-free port of the subset
of build_image.sh's Bash/python3 helper code that PhotoCraft needs.

    docker/feishu_record.py --component photocraft-web --tag amd_20261013_deadbee \
        --sheets AMD_with_mxn100,AMD_with_cuda --date 20261013

    docker/feishu_record.py --component photocraft-web --tag ... --sheets ... --porcelain

--porcelain prints one "<sheet>\t<component>\t<tag>" line per sheet and makes no changes, for
dry runs on a workstation that cannot reach Feishu.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

DEFAULT_SPREADSHEET = os.environ.get("FEISHU_SPREADSHEET_TOKEN", "Htotsn3oahO1zxt73YMcaB1zn8e")
CONFIG_FILES = ("~/.feishu.components.json", "~/.feishu.json")
FEISHU_BASE = "https://open.feishu.cn/open-apis"


class Failure(Exception):
    """Anything the operator must act on: bad credentials, missing sheet, refused write."""


def log(message: str) -> None:
    print("[feishu] " + message)


def read_credentials() -> tuple[str, str]:
    configured = [os.environ.get("FEISHU_CONFIG_FILE"), os.environ.get("FEISHU_FALLBACK_CONFIG_FILE")]
    paths = [path for path in configured if path] + [os.path.expanduser(p) for p in CONFIG_FILES]
    seen: set[str] = set()
    for path in paths:
        if not path or path in seen:
            continue
        seen.add(path)
        try:
            with open(path, encoding="utf-8") as handle:
                data = json.load(handle)
        except (OSError, ValueError) as error:
            log("skip unreadable credentials " + path + ": " + str(error))
            continue
        app_id = str(data.get("feishu_app_id", "")).strip()
        app_secret = str(data.get("feishu_app_secret", "")).strip()
        if app_id and app_secret:
            log("using credentials from " + path)
            return app_id, app_secret
    raise Failure("no readable Feishu credentials (set FEISHU_CONFIG_FILE or ~/.feishu.json)")


def request(method: str, url: str, token: str | None = None, payload: dict | None = None) -> dict:
    body = json.dumps(payload).encode("utf-8") if payload is not None else None
    headers = {"Content-Type": "application/json; charset=utf-8"}
    if token:
        headers["Authorization"] = "Bearer " + token
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            data = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", "replace")[:300]
        raise Failure(f"{method} {url} failed: HTTP {error.code} {detail}") from error
    except urllib.error.URLError as error:
        raise Failure(f"{method} {url} failed: {error.reason}") from error
    if not isinstance(data, dict):
        raise Failure(f"{method} {url} returned no JSON object")
    return data


def tenant_token(app_id: str, app_secret: str) -> str:
    data = request(
        "POST",
        FEISHU_BASE + "/auth/v3/tenant_access_token/internal",
        payload={"app_id": app_id, "app_secret": app_secret},
    )
    if data.get("code") != 0:
        raise Failure("tenant token refused: " + str(data))
    token = data.get("tenant_access_token")
    if not token:
        raise Failure("tenant token missing from response")
    return str(token)


def sheet_id(token: str, spreadsheet: str, title: str) -> str:
    url = FEISHU_BASE + "/sheets/v3/spreadsheets/" + urllib.parse.quote(spreadsheet) + "/sheets/query"
    data = request("GET", url, token)
    if data.get("code") != 0:
        raise Failure("sheet query failed: " + str(data))
    for sheet in data.get("data", {}).get("sheets", []):
        if sheet.get("title") == title:
            return str(sheet["sheet_id"])
    raise Failure("no sheet titled " + repr(title) + " in the spreadsheet")


def values(token: str, spreadsheet: str, sheet: str, cell_range: str) -> list[list]:
    url = (
        FEISHU_BASE
        + "/sheets/v2/spreadsheets/"
        + urllib.parse.quote(spreadsheet)
        + "/values/"
        + urllib.parse.quote(sheet + "!" + cell_range, safe="!:")
    )
    data = request("GET", url, token)
    if data.get("code") != 0:
        raise Failure("read " + cell_range + " failed: " + str(data))
    return data.get("data", {}).get("valueRange", {}).get("values", []) or []


def put_value(token: str, spreadsheet: str, sheet: str, cell: str, value: str) -> None:
    url = FEISHU_BASE + "/sheets/v2/spreadsheets/" + urllib.parse.quote(spreadsheet) + "/values"
    payload = {"valueRange": {"range": sheet + "!" + cell + ":" + cell, "values": [[value]]}}
    data = request("PUT", url, token, payload)
    if data.get("code") != 0:
        raise Failure("write " + cell + " failed: " + str(data))


def prepend_row(token: str, spreadsheet: str, sheet: str, date: str) -> None:
    url = FEISHU_BASE + "/sheets/v2/spreadsheets/" + urllib.parse.quote(spreadsheet) + "/values_prepend"
    payload = {"valueRange": {"range": sheet + "!A4:A4", "values": [[date]]}}
    data = request("POST", url, token, payload)
    if data.get("code") != 0:
        raise Failure("prepend the date row failed: " + str(data))


def column_letter(index: int) -> str:
    name = ""
    while index:
        index, remainder = divmod(index - 1, 26)
        name = chr(ord("A") + remainder) + name
    return name


def cell_text(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        return value.strip()
    if isinstance(value, dict):
        return str(value.get("text") or value.get("link") or "").strip()
    if isinstance(value, list):
        return "".join(cell_text(item) for item in value).strip()
    return str(value).strip()


def find_column(token: str, spreadsheet: str, sheet: str, component: str) -> int | None:
    rows = values(token, spreadsheet, sheet, "A1:ZZ1")
    header = rows[0] if rows else []
    for index, cell in enumerate(header, start=1):
        if cell_text(cell) == component:
            return index
    return None


def find_date_row(token: str, spreadsheet: str, sheet: str, date: str) -> int | None:
    for offset, row in enumerate(values(token, spreadsheet, sheet, "A4:A2000")):
        if row and cell_text(row[0]) == date:
            return offset + 4
    return None


def insert_column(token: str, spreadsheet: str, sheet: str, component: str, repository: str) -> int:
    """Add the component's column after the last used one and write its header and repository."""
    rows = values(token, spreadsheet, sheet, "A1:ZZ2")
    last = 0
    for row in rows:
        for index, cell in enumerate(row, start=1):
            if cell_text(cell):
                last = max(last, index)

    url = FEISHU_BASE + "/sheets/v2/spreadsheets/" + urllib.parse.quote(spreadsheet) + "/insert_dimension_range"
    payload = {
        "dimension": {"sheetId": sheet, "majorDimension": "COLUMNS", "startIndex": last, "endIndex": last + 1},
        "inheritStyle": "BEFORE",
    }
    data = request("POST", url, token, payload)
    if data.get("code") != 0:
        raise Failure("insert a column for " + component + " failed: " + str(data))

    letter = column_letter(last + 1)
    put_value(token, spreadsheet, sheet, letter + "1", component)
    put_value(token, spreadsheet, sheet, letter + "2", repository)
    return last + 1


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--component", required=True, help="row 1 header cell, e.g. photocraft-web")
    parser.add_argument("--tag", required=True, help="the tag to record, e.g. amd_20261013_deadbee")
    parser.add_argument("--sheets", required=True, help="comma-separated sheet titles")
    parser.add_argument("--date", default="", help="row key (YYYYMMDD); today (UTC) when omitted")
    parser.add_argument("--spreadsheet", default=DEFAULT_SPREADSHEET)
    parser.add_argument("--repository", default=os.environ.get("IMAGE_REPO", ""), help="row 2 of a new column")
    parser.add_argument("--porcelain", action="store_true", help="print what would be written and exit")
    args = parser.parse_args(argv)

    date = args.date or __import__("datetime").datetime.now(__import__("datetime").timezone.utc).strftime("%Y%m%d")
    sheets = [sheet.strip() for sheet in args.sheets.split(",") if sheet.strip()]
    if not sheets:
        raise Failure("--sheets is empty")
    if args.porcelain:
        for sheet in sheets:
            print(f"{sheet}\t{args.component}\t{args.tag}\t{args.date or date}")
        return 0

    app_id, app_secret = read_credentials()
    token = tenant_token(app_id, app_secret)
    for sheet_title in sheets:
        sheet = sheet_id(token, args.spreadsheet, sheet_title)
        column = find_column(token, args.spreadsheet, sheet, args.component)
        if column is None:
            if not args.repository:
                raise Failure(sheet_title + ": no column for " + args.component + " and --repository is empty")
            column = insert_column(token, args.spreadsheet, sheet, args.component, args.repository)
            log(sheet_title + ": created the " + args.component + " column")
        row = find_date_row(token, args.spreadsheet, sheet, date)
        if row is None:
            prepend_row(token, args.spreadsheet, sheet, date)
            row = 4
            log(sheet_title + ": created the " + date + " row")
        put_value(token, args.spreadsheet, sheet, column_letter(column) + str(row), args.tag)
        log(f"{sheet_title}!{column_letter(column)}{row} = {args.tag}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except Failure as failure:
        print("[feishu][ERROR] " + str(failure), file=sys.stderr)
        sys.exit(1)
