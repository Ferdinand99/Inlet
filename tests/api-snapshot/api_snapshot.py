#!/usr/bin/env python3
"""Keeps Inlet honest about the Fluxer API it depends on.

Inlet calls a fixed set of REST routes (method + path). This tool

  * derives that set from the `inlet-protocol` sources (`routes`),
  * extracts those routes, with the request and response schemas they reference,
    from a Fluxer OpenAPI document into small, sorted, stable JSON (`update`),
  * compares the live document of an instance with the committed snapshot (`check`).

Standard library only. The OpenAPI document is public and is fetched without any
credential; it is treated as untrusted input: it is size capped, parsed as JSON and
never evaluated, and nothing from it is passed to a shell.

Exit codes: 0 no drift, 1 drift, 2 internal error or stale route list, 3 fetch error.
"""

import argparse
import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
PROTOCOL_SRC = REPO / "crates" / "inlet-protocol" / "src"
EXTRA_ROUTES = HERE / "routes-extra.txt"

# The instances Inlet is tested against. Origins only; the API address comes from
# the instance's own discovery document, as in the client.
INSTANCES = {
    "fluxer.app": "https://fluxer.app",
    "fluxer.opland.net": "https://fluxer.opland.net",
}

MAX_BYTES = 16 * 1024 * 1024  # a document is about 1.5 MiB today
TIMEOUT_SECS = 30
MAX_REDIRECTS = 3
USER_AGENT = "inlet-api-drift (+https://github.com/Ferdinand99/Inlet)"
METHODS = ("GET", "POST", "PUT", "PATCH", "DELETE")
# Helpers in inlet-protocol that fix the HTTP method for the path they are given.
HELPER_METHODS = {
    "get_list": "GET",
    "get_list_capped": "GET",
    "archived_path": "GET",
    "post_with_captcha": "POST",
    "send_with_captcha": None,  # takes the method as its first argument
    "raw_upload": "POST",
}
# Keys that carry prose, not contract. Dropped so a reworded description is no drift.
NOISE_KEYS = {"description", "title", "summary", "example", "examples", "externalDocs"}
MAX_DIFF_LINES = 400
MAX_ISSUE_CHARS = 60000

REF_PREFIX = "#/components/schemas/"


class FetchError(Exception):
    pass


class ToolError(Exception):
    pass


# ---------------------------------------------------------------------------
# Routes used by inlet-protocol
# ---------------------------------------------------------------------------


def normalize_path(path):
    """`/a/{x}/b?q=1` -> `/a/{}/b`: parameter names and the query do not matter."""
    path = path.split("?", 1)[0]
    return re.sub(r"\{[^{}]*\}", "{}", path)


def mask_rust(src):
    """Blanks comments and string contents (same length), and lists the strings.

    Returns (masked, strings) with strings as (start, end, content) for ordinary and
    raw string literals. Positions in `masked` equal positions in `src`.
    """
    out = list(src)
    strings = []
    i, n = 0, len(src)

    def blank(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c == "r" and re.match(r'r#*"', src[i:i + 40]) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            m = re.match(r'r(#*)"', src[i:])
            hashes = m.group(1)
            start = i + m.end()
            end = src.find('"' + hashes, start)
            if end < 0:
                raise ToolError("unterminated raw string")
            strings.append((i, end + 1 + len(hashes), src[start:end]))
            blank(i, end + 1 + len(hashes))
            i = end + 1 + len(hashes)
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            if j >= n:
                raise ToolError("unterminated string")
            strings.append((i, j + 1, src[i + 1:j]))
            blank(i, j + 1)
            i = j + 1
        elif c == "'":
            m = re.match(r"'(?:\\.[^']*|[^\\'])'", src[i:i + 12])
            if m:  # a char literal; a lifetime has no closing quote
                blank(i, i + m.end())
                i += m.end()
            else:
                i += 1
        else:
            i += 1
    return "".join(out), strings


def test_spans(masked):
    """Spans of items behind `#[cfg(test)]` (a `mod tests { .. }` or a single item)."""
    spans = []
    for m in re.finditer(r"#\[cfg\(test\)\]", masked):
        j = m.end()
        while j < len(masked) and masked[j] not in "{;":
            j += 1
        if j >= len(masked):
            continue
        if masked[j] == ";":
            end = j + 1
        else:
            depth, k = 0, j
            while k < len(masked):
                if masked[k] == "{":
                    depth += 1
                elif masked[k] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                k += 1
            end = k + 1
        spans.append((m.start(), end))
    return spans


def is_api_path(text):
    return (
        re.fullmatch(r"/[A-Za-z0-9@_{}\-][A-Za-z0-9@_{}\-./]*(\?[^\s\"]*)?", text) is not None
        and not text.startswith("/.well-known")
    )


def scan_source(src_dir=PROTOCOL_SRC):
    """Returns (resolved, unresolved) from the production code of inlet-protocol.

    `resolved` maps (METHOD, normalized path) to a sorted list of `file:line`.
    `unresolved` maps a normalized path literal whose method cannot be read off the
    call site (a path another function builds on) to `file:line`.
    """
    resolved, unresolved = {}, {}
    for file in sorted(src_dir.rglob("*.rs")):
        src = file.read_text(encoding="utf-8")
        masked, strings = mask_rust(src)
        spans = test_spans(masked)
        rel = file.relative_to(REPO).as_posix()
        lits = [
            (a, b, s)
            for (a, b, s) in strings
            if is_api_path(s) and not any(x <= a < y for x, y in spans)
        ]
        for idx, (a, b, text) in enumerate(lits):
            line = src.count("\n", 0, a) + 1
            where = f"{rel}:{line}"
            path = normalize_path(text)
            nxt = next(
                (a2 for (a2, _b2, t2) in lits[idx + 1:] if normalize_path(t2) != path),
                len(masked),
            )
            method = method_for(masked, a, b, nxt)
            if method is None:
                unresolved.setdefault(path, where)
            else:
                resolved.setdefault((method, path), []).append(where)
    return resolved, unresolved


def method_for(masked, start, end, next_literal_start):
    """The HTTP method for the path literal at [start, end), or None."""
    # Backward, inside the same statement.
    fns = [m.start() for m in re.finditer(r"\bfn\b", masked[:start])]
    stmt = max(masked.rfind(";", 0, start), fns[-1] if fns else -1)
    before = masked[stmt + 1:start]
    found = None
    for m in re.finditer(r"Method::(GET|POST|PUT|PATCH|DELETE)\b", before):
        found = m.group(1)
    if found:
        return found
    for m in re.finditer(r"\b([a-z_]+)\s*\(", before):
        if m.group(1) in HELPER_METHODS and HELPER_METHODS[m.group(1)]:
            found = HELPER_METHODS[m.group(1)]
    if found:
        return found
    # Forward: `let path = format!("/x");` and then `raw(Method::GET, &path, ..)`.
    # Stops at the next function or the next path literal.
    limit = min(next_literal_start, len(masked))
    fn_at = re.search(r"\bfn\b", masked[end:limit])
    if fn_at:
        limit = end + fn_at.start()
    after = masked[end:limit]
    hits = []
    m = re.search(r"Method::(GET|POST|PUT|PATCH|DELETE)\b", after)
    if m:
        hits.append((m.start(), m.group(1)))
    for m in re.finditer(r"\b([a-z_]+)\s*\(", after):
        if HELPER_METHODS.get(m.group(1)):
            hits.append((m.start(), HELPER_METHODS[m.group(1)]))
            break
    return min(hits)[1] if hits else None


def load_extra_routes():
    routes = set()
    if not EXTRA_ROUTES.exists():
        return routes
    for raw in EXTRA_ROUTES.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        method, _, path = line.partition(" ")
        if method not in METHODS or not path.startswith("/"):
            raise ToolError(f"routes-extra.txt: bad line {raw!r}")
        routes.add((method, normalize_path(path.strip())))
    return routes


def inlet_routes():
    """Every (METHOD, path) Inlet calls. Raises ToolError when the extra list is stale."""
    resolved, unresolved = scan_source()
    extra = load_extra_routes()
    problems = []
    for path, where in sorted(unresolved.items()):
        # A literal with no method at its call site is a base that another function
        # extends (reactions). It must be covered by an entry in routes-extra.txt.
        if not any(p == path or p.startswith(path + "/") for _, p in extra):
            problems.append(
                f"{where}: path {path} has no HTTP method at its call site and no "
                f"entry in routes-extra.txt"
            )
    resolved_paths = {p for _, p in resolved}
    literal_paths = resolved_paths | set(unresolved)
    for method, path in sorted(extra):
        if not any(path == lp or path.startswith(lp + "/") for lp in literal_paths):
            problems.append(f"routes-extra.txt: {method} {path} matches no path in inlet-protocol")
    if len(resolved) + len(extra) < 20:
        problems.append(f"only {len(resolved) + len(extra)} routes found: the scanner is broken")
    if problems:
        raise ToolError("\n".join(problems))
    return sorted(set(resolved) | extra)


# ---------------------------------------------------------------------------
# OpenAPI extraction
# ---------------------------------------------------------------------------


def norm(node, refs):
    """A schema or other node without prose; `$ref`s become names and are collected."""
    if isinstance(node, dict):
        ref = node.get("$ref")
        if isinstance(ref, str):
            if not ref.startswith(REF_PREFIX):
                raise ToolError(f"unexpected $ref {ref!r}")
            name = ref[len(REF_PREFIX):]
            refs.add(name)
            return {"$ref": name}
        return {
            k: norm(v, refs)
            for k, v in sorted(node.items())
            if k not in NOISE_KEYS
        }
    if isinstance(node, list):
        items = [norm(v, refs) for v in node]
        # Order carries no meaning in these.
        if all(isinstance(v, (str, int, float, bool)) or v is None for v in items):
            return sorted(items, key=lambda v: json.dumps(v))
        return sorted(items, key=lambda v: json.dumps(v, sort_keys=True))
    return node


def operation_view(op, refs):
    params = []
    for p in op.get("parameters", []):
        if "$ref" in p:
            raise ToolError("parameter $ref is not handled")
        params.append(
            {
                "in": p.get("in"),
                "name": p.get("name"),
                "required": bool(p.get("required", False)),
                "schema": norm(p.get("schema", {}), refs),
            }
        )
    params.sort(key=lambda p: (str(p["in"]), str(p["name"])))
    view = {"operationId": op.get("operationId"), "parameters": params}
    body = op.get("requestBody")
    if body:
        view["requestBody"] = {
            "required": bool(body.get("required", False)),
            "content": {
                mime: norm(c.get("schema", {}), refs)
                for mime, c in sorted(body.get("content", {}).items())
            },
        }
    responses = {}
    for code, resp in sorted(op.get("responses", {}).items()):
        if not str(code).startswith("2"):
            continue  # the error bodies are the same generic ones on every route
        responses[str(code)] = {
            mime: norm(c.get("schema", {}), refs)
            for mime, c in sorted(resp.get("content", {}).items())
        }
    view["responses"] = responses
    return view


def extract(doc, routes):
    """The snapshot of `routes` taken from an OpenAPI document."""
    if not isinstance(doc, dict) or not isinstance(doc.get("paths"), dict):
        raise FetchError("the document has no `paths` object")
    schemas = doc.get("components", {}).get("schemas", {})
    by_norm = {}
    for path in doc["paths"]:
        key = normalize_path(path)
        if key in by_norm:
            raise FetchError(f"two documented paths look alike: {by_norm[key]} and {path}")
        by_norm[key] = path
    refs, out_routes = set(), {}
    for method, path in routes:
        label = f"{method} {path}"
        doc_path = by_norm.get(path)
        op = None
        if doc_path is not None:
            op = doc["paths"][doc_path].get(method.lower())
        if not isinstance(op, dict):
            out_routes[label] = {"missing": True}
            continue
        view = operation_view(op, refs)
        view["documentedPath"] = doc_path
        out_routes[label] = view
    out_schemas, queue = {}, sorted(refs)
    while queue:
        name = queue.pop()
        if name in out_schemas:
            continue
        schema = schemas.get(name)
        if schema is None:
            out_schemas[name] = {"missing": True}
            continue
        inner = set()
        out_schemas[name] = norm(schema, inner)
        queue.extend(sorted(inner - set(out_schemas)))
    return {
        "routes": dict(sorted(out_routes.items())),
        "schemas": dict(sorted(out_schemas.items())),
    }


def dump(snapshot):
    """Stable text: sorted keys, two-space indent, one trailing newline."""
    return json.dumps(snapshot, indent=1, sort_keys=True, ensure_ascii=False) + "\n"


# ---------------------------------------------------------------------------
# Diff
# ---------------------------------------------------------------------------


def short(value):
    text = json.dumps(value, sort_keys=True, ensure_ascii=False)
    return text if len(text) <= 200 else text[:197] + "..."


def diff_nodes(old, new, path, out):
    if old == new:
        return
    if isinstance(old, dict) and isinstance(new, dict):
        for key in sorted(set(old) | set(new)):
            sub = f"{path}.{key}" if path else key
            if key not in new:
                out.append(f"- {sub}: {short(old[key])}")
            elif key not in old:
                out.append(f"+ {sub}: {short(new[key])}")
            else:
                diff_nodes(old[key], new[key], sub, out)
    else:
        out.append(f"- {path}: {short(old)}")
        out.append(f"+ {path}: {short(new)}")


def diff_snapshots(old, new):
    out = []
    diff_nodes(old, new, "", out)
    return out


# ---------------------------------------------------------------------------
# Fetching
# ---------------------------------------------------------------------------


class _HttpsOnlyRedirects(urllib.request.HTTPRedirectHandler):
    max_redirections = MAX_REDIRECTS

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not newurl.lower().startswith("https://"):
            raise FetchError(f"redirect to a non-https address refused: {newurl[:100]}")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def http_get_json(url):
    if not url.lower().startswith("https://"):
        raise FetchError(f"not an https address: {url[:100]}")
    req = urllib.request.Request(
        url, headers={"User-Agent": USER_AGENT, "Accept": "application/json"}, method="GET"
    )
    opener = urllib.request.build_opener(_HttpsOnlyRedirects)
    try:
        with opener.open(req, timeout=TIMEOUT_SECS) as resp:
            if resp.status != 200:
                raise FetchError(f"HTTP {resp.status} from {url}")
            body = resp.read(MAX_BYTES + 1)
    except FetchError:
        raise
    except urllib.error.HTTPError as e:
        raise FetchError(f"HTTP {e.code} from {url}") from None
    except (urllib.error.URLError, OSError, ValueError) as e:
        raise FetchError(f"cannot fetch {url}: {e.__class__.__name__}") from None
    if len(body) > MAX_BYTES:
        raise FetchError(f"{url} is larger than {MAX_BYTES} bytes")
    try:
        return json.loads(body)
    except ValueError:
        raise FetchError(f"{url} is not JSON") from None


def fetch_openapi(origin):
    discovery = http_get_json(origin.rstrip("/") + "/.well-known/fluxer")
    try:
        api = discovery["endpoints"]["api_public"]
    except (KeyError, TypeError):
        raise FetchError("discovery has no endpoints.api_public") from None
    if not isinstance(api, str) or not api.startswith("https://"):
        raise FetchError("endpoints.api_public is not an https address")
    return http_get_json(api.rstrip("/") + "/v1/openapi.json")


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


def snapshot_file(instance):
    return HERE / f"{instance}.json"


def cmd_routes(_args):
    for method, path in inlet_routes():
        print(f"{method} {path}")
    return 0


def cmd_update(args):
    routes = inlet_routes()
    for instance in args.instances or list(INSTANCES):
        doc = load_doc(instance, args.from_file)
        snap = extract(doc, routes)
        snapshot_file(instance).write_text(dump(snap), encoding="utf-8", newline="\n")
        missing = [k for k, v in snap["routes"].items() if v.get("missing")]
        print(f"{instance}: {len(snap['routes'])} routes, {len(snap['schemas'])} schemas written")
        for k in missing:
            print(f"  not documented: {k}")
    return 0


def load_doc(instance, from_file):
    if from_file:
        return json.loads(Path(from_file).read_text(encoding="utf-8"))
    return fetch_openapi(INSTANCES[instance])


def cmd_check(args):
    routes = inlet_routes()
    instance = args.instance
    committed_path = snapshot_file(instance)
    if not committed_path.exists():
        raise ToolError(f"no snapshot for {instance}: run `update {instance}`")
    committed = json.loads(committed_path.read_text(encoding="utf-8"))
    doc = load_doc(instance, args.from_file)
    live = extract(doc, routes)
    lines = diff_snapshots(committed, live)
    if not lines:
        print(f"{instance}: no drift ({len(live['routes'])} routes, {len(live['schemas'])} schemas)")
        return 0
    shown = lines[:MAX_DIFF_LINES]
    print(f"{instance}: API drift, {len(lines)} changed lines")
    print("\n".join(shown))
    if len(lines) > len(shown):
        print(f"... {len(lines) - len(shown)} more lines")
    if args.issue_body:
        write_issue_body(Path(args.issue_body), instance, shown, len(lines))
    return 1


def write_issue_body(path, instance, shown, total):
    text = "\n".join(shown)
    digest = hashlib.sha256("\n".join(shown).encode("utf-8")).hexdigest()[:16]
    # A fence longer than any run of backticks in the (untrusted) text keeps the
    # block closed whatever the document contains.
    longest = max((len(m) for m in re.findall(r"`+", text)), default=0)
    fence = "`" * max(3, longest + 1)
    more = f"\n... {total - len(shown)} more lines" if total > len(shown) else ""
    body = (
        f"The public OpenAPI document of `{instance}` no longer matches the committed "
        f"snapshot of the routes Inlet uses (`tests/api-snapshot/{instance}.json`).\n\n"
        f"{total} changed lines. `-` is the snapshot, `+` is the live document.\n\n"
        f"<details><summary>Diff</summary>\n\n{fence}diff\n{text}{more}\n{fence}\n\n</details>\n\n"
        f"After updating Inlet for the change, refresh the snapshot as described in "
        f"`docs/API-DRIFT.md`.\n\n<!-- drift-hash: {digest} -->\n"
    )
    if len(body) > MAX_ISSUE_CHARS:
        keep = MAX_ISSUE_CHARS - 2000
        cut = "\n".join(shown)[:keep].rsplit("\n", 1)[0]
        return write_issue_body(path, instance, cut.split("\n"), total)
    path.write_text(body, encoding="utf-8", newline="\n")
    return None


def cmd_verify(_args):
    """Offline: the route list is complete and each committed snapshot covers it."""
    routes = inlet_routes()
    wanted = sorted(f"{m} {p}" for m, p in routes)
    bad = 0
    for instance in INSTANCES:
        f = snapshot_file(instance)
        if not f.exists():
            print(f"{instance}: snapshot missing", file=sys.stderr)
            bad = 1
            continue
        have = sorted(json.loads(f.read_text(encoding="utf-8"))["routes"])
        if have != wanted:
            print(f"{instance}: the snapshot's route list differs from inlet-protocol:", file=sys.stderr)
            for r in sorted(set(wanted) - set(have)):
                print(f"  + {r}", file=sys.stderr)
            for r in sorted(set(have) - set(wanted)):
                print(f"  - {r}", file=sys.stderr)
            bad = 1
    if not bad:
        print(f"ok: {len(wanted)} routes, snapshots cover them")
    return 2 if bad else 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("routes", help="print the routes inlet-protocol uses").set_defaults(fn=cmd_routes)
    sub.add_parser("verify", help="offline: snapshots cover the route list").set_defaults(fn=cmd_verify)
    up = sub.add_parser("update", help="rewrite the snapshots from the live documents")
    up.add_argument("instances", nargs="*", choices=list(INSTANCES))
    up.add_argument("--from-file", help="read the document from a file instead of fetching")
    up.set_defaults(fn=cmd_update)
    ck = sub.add_parser("check", help="compare the live document with the snapshot")
    ck.add_argument("instance", choices=list(INSTANCES))
    ck.add_argument("--from-file", help="read the document from a file instead of fetching")
    ck.add_argument("--issue-body", help="write a GitHub issue body here when there is drift")
    ck.set_defaults(fn=cmd_check)
    args = ap.parse_args(argv)
    try:
        return args.fn(args)
    except FetchError as e:
        print(f"fetch failed: {e}", file=sys.stderr)
        return 3
    except ToolError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    except (OSError, ValueError, KeyError, TypeError) as e:
        # Never let a crash end in Python's exit code 1, which means drift here.
        print(f"error: {e.__class__.__name__}: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
