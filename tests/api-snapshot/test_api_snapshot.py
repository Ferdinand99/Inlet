"""Tests for api_snapshot.py. Run: python3 -I -m unittest discover -s tests/api-snapshot"""

import io
import json
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

import api_snapshot as tool

DOC = {
    "openapi": "3.1.0",
    "paths": {
        "/things/{thing_id}": {
            "get": {
                "operationId": "get_thing",
                "summary": "Reworded text must not matter",
                "parameters": [
                    {"name": "thing_id", "in": "path", "required": True,
                     "schema": {"$ref": "#/components/schemas/Id", "description": "x"}},
                ],
                "responses": {
                    "200": {"description": "ok", "content": {"application/json": {
                        "schema": {"$ref": "#/components/schemas/Thing"}}}},
                    "429": {"content": {"application/json": {
                        "schema": {"$ref": "#/components/schemas/Throttled"}}}},
                },
            },
            "delete": {"operationId": "delete_thing", "responses": {"204": {"description": "gone"}}},
        },
        "/things": {
            "post": {
                "operationId": "make_thing",
                "requestBody": {"required": True, "content": {"application/json": {
                    "schema": {"$ref": "#/components/schemas/Thing"}}}},
                "responses": {"200": {"content": {"application/json": {
                    "schema": {"$ref": "#/components/schemas/Thing"}}}}},
            },
        },
        "/unused": {"get": {"operationId": "unused", "responses": {"200": {}}}},
    },
    "components": {"schemas": {
        "Id": {"type": "string", "pattern": "^[0-9]+$", "description": "prose"},
        "Thing": {"type": "object", "required": ["b", "a"], "properties": {
            "a": {"$ref": "#/components/schemas/Id"},
            "b": {"type": "string", "enum": ["y", "x"]},
            "child": {"$ref": "#/components/schemas/Thing"},  # a cycle
        }},
        "Throttled": {"type": "object"},
        "Unused": {"type": "string"},
    }},
}
ROUTES = [("DELETE", "/things/{}"), ("GET", "/things/{}"), ("POST", "/things"), ("GET", "/gone")]


class ScannerTests(unittest.TestCase):
    def scan(self, files):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            for name, text in files.items():
                (root / name).write_text(text, encoding="utf-8")
            with mock.patch.object(tool, "REPO", root):
                return tool.scan_source(root)

    def test_method_before_and_after_the_path_and_helpers(self):
        resolved, unresolved = self.scan({"a.rs": '''
            fn a() { self.raw(Method::POST, &format!("/a/{}/b", id)?, None); }
            fn b() {
                let path = format!("/c/{x}?q={}", q);
                self.raw(Method::PATCH, &path, None);
            }
            fn c() { self.get_list(&format!("/d/{}", id), "d"); }
            fn d() { self.send_with_captcha(Method::PUT, "/e", None); }
            fn e() -> String { format!("/base/{}/x", id) }
            fn f() { self.raw(Method::GET, "/ignored", None) } // "/in-comment"
        '''})
        self.assertEqual(
            sorted(resolved),
            [("GET", "/d/{}"), ("GET", "/ignored"), ("PATCH", "/c/{}"), ("POST", "/a/{}/b"), ("PUT", "/e")],
        )
        self.assertEqual(list(unresolved), ["/base/{}/x"])

    def test_test_code_and_comments_are_not_routes(self):
        resolved, unresolved = self.scan({"a.rs": '''
            /// `GET /doc/comment`
            fn a() { self.raw(Method::GET, "/real", None); }
            #[cfg(test)]
            mod tests {
                fn t() { archived_path("/x", None); let u = "/tested/{}"; }
            }
            #[cfg(test)]
            fn helper() { let _ = "/helper-only"; }
            fn after() { self.raw(Method::DELETE, "/after-tests", None); }
        '''})
        self.assertEqual(sorted(resolved), [("DELETE", "/after-tests"), ("GET", "/real")])
        self.assertEqual(unresolved, {})

    def test_strings_that_look_like_code_do_not_confuse_the_scanner(self):
        resolved, _ = self.scan({"a.rs": r'''
            fn a() {
                let q = '"'; let s = "a \" quote; fn Method::GET"; let r = r#"x "/not" y"#;
                self.raw(Method::PUT, "/ok/{}", None);
            }
        '''})
        self.assertEqual(sorted(resolved), [("PUT", "/ok/{}")])

    def test_the_real_route_list_is_complete_and_the_snapshots_cover_it(self):
        out = io.StringIO()
        err = io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = tool.main(["verify"])
        self.assertEqual(code, 0, err.getvalue())
        routes = tool.inlet_routes()
        self.assertIn(("POST", "/channels/{}/messages"), routes)
        self.assertIn(("PUT", "/channels/{}/messages/{}/reactions/{}/@me"), routes)

    def test_a_stale_extra_route_fails(self):
        with tempfile.TemporaryDirectory() as d:
            extra = Path(d) / "extra.txt"
            extra.write_text("GET /no/such/path\n", encoding="utf-8")
            with mock.patch.object(tool, "EXTRA_ROUTES", extra):
                with self.assertRaises(tool.ToolError):
                    tool.inlet_routes()


class ExtractTests(unittest.TestCase):
    def test_snapshot_holds_only_used_routes_and_what_they_reference(self):
        snap = tool.extract(DOC, ROUTES)
        self.assertEqual(
            sorted(snap["routes"]),
            ["DELETE /things/{}", "GET /gone", "GET /things/{}", "POST /things"],
        )
        self.assertEqual(snap["routes"]["GET /gone"], {"missing": True})
        self.assertEqual(sorted(snap["schemas"]), ["Id", "Thing"])  # no Throttled, no Unused
        get = snap["routes"]["GET /things/{}"]
        self.assertEqual(get["responses"], {"200": {"application/json": {"$ref": "Thing"}}})
        self.assertEqual(get["parameters"][0]["schema"], {"$ref": "Id"})
        self.assertEqual(snap["schemas"]["Thing"]["required"], ["a", "b"])
        self.assertEqual(snap["schemas"]["Thing"]["properties"]["b"]["enum"], ["x", "y"])
        self.assertNotIn("description", json.dumps(snap))
        self.assertNotIn("summary", json.dumps(snap))

    def test_output_is_stable(self):
        a = tool.dump(tool.extract(DOC, ROUTES))
        shuffled = json.loads(json.dumps(DOC))
        shuffled["paths"] = dict(reversed(list(shuffled["paths"].items())))
        b = tool.dump(tool.extract(shuffled, list(reversed(ROUTES))))
        self.assertEqual(a, b)
        self.assertTrue(a.endswith("}\n"))

    def test_bad_documents_are_fetch_errors(self):
        with self.assertRaises(tool.FetchError):
            tool.extract([], ROUTES)
        with self.assertRaises(tool.FetchError):
            tool.extract({"paths": {"/a/{x}": {}, "/a/{y}": {}}}, ROUTES)

    def test_a_foreign_ref_is_refused(self):
        doc = json.loads(json.dumps(DOC))
        doc["components"]["schemas"]["Id"] = {"$ref": "https://evil.example/x"}
        with self.assertRaises(tool.ToolError):
            tool.extract(doc, ROUTES)


class DiffAndCheckTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        root = Path(self.dir.name)
        self.snap = root / "snap"
        self.snap.mkdir()
        self.live = root / "live.json"
        patches = [
            mock.patch.object(tool, "snapshot_file", lambda inst: self.snap / f"{inst}.json"),
            mock.patch.object(tool, "inlet_routes", lambda: ROUTES),
        ]
        for p in patches:
            p.start()
            self.addCleanup(p.stop)
        (self.snap / "fluxer.app.json").write_text(
            tool.dump(tool.extract(DOC, ROUTES)), encoding="utf-8")

    def run_check(self, doc, *extra):
        self.live.write_text(json.dumps(doc), encoding="utf-8")
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = tool.main(["check", "fluxer.app", "--from-file", str(self.live), *extra])
        return code, out.getvalue(), err.getvalue()

    def test_no_change_and_prose_change_are_no_drift(self):
        code, out, _ = self.run_check(DOC)
        self.assertEqual(code, 0)
        self.assertIn("no drift", out)
        reworded = json.loads(json.dumps(DOC))
        reworded["paths"]["/things/{thing_id}"]["get"]["summary"] = "Something else"
        reworded["components"]["schemas"]["Id"]["description"] = "other"
        # Changes outside the routes in use are no drift either.
        reworded["paths"]["/unused"]["get"]["operationId"] = "renamed"
        self.assertEqual(self.run_check(reworded)[0], 0)

    def test_schema_route_and_method_changes_are_reported(self):
        changed = json.loads(json.dumps(DOC))
        changed["components"]["schemas"]["Thing"]["properties"]["b"]["enum"] = ["x", "z"]
        del changed["paths"]["/things"]["post"]
        body = Path(self.dir.name) / "issue.md"
        code, out, _ = self.run_check(changed, "--issue-body", str(body))
        self.assertEqual(code, 1)
        self.assertIn("POST /things", out)
        self.assertIn("missing", out)
        self.assertIn("schemas.Thing.properties.b.enum", out)
        text = body.read_text(encoding="utf-8")
        self.assertIn("<details>", text)
        self.assertIn("drift-hash:", text)

    def test_the_issue_body_cannot_be_broken_out_of(self):
        evil = json.loads(json.dumps(DOC))
        evil["components"]["schemas"]["Id"]["pattern"] = "```\n</details> @everyone ````` x"
        body = Path(self.dir.name) / "issue.md"
        self.run_check(evil, "--issue-body", str(body))
        text = body.read_text(encoding="utf-8")
        opening = [l for l in text.splitlines() if l.endswith("diff") and l.startswith("`")]
        fence = opening[0][: -len("diff")]
        self.assertGreater(len(fence), 5)  # longer than the longest backtick run inside
        self.assertEqual(sum(1 for l in text.splitlines() if l == fence), 1)

    def test_a_crash_is_never_reported_as_drift(self):
        code, _, err = self.run_check(DOC, "--from-file", str(Path(self.dir.name) / "nosuch.json"))
        self.assertEqual(code, 2)
        self.assertIn("error:", err)

    def test_the_issue_body_is_capped(self):
        big = json.loads(json.dumps(DOC))
        big["components"]["schemas"]["Thing"]["properties"] = {
            f"p{i:05d}": {"type": "string", "pattern": "x" * 150} for i in range(2000)
        }
        body = Path(self.dir.name) / "issue.md"
        code, out, _ = self.run_check(big, "--issue-body", str(body))
        self.assertEqual(code, 1)
        self.assertLessEqual(len(body.read_text(encoding="utf-8")), tool.MAX_ISSUE_CHARS)
        self.assertLessEqual(out.count("\n"), tool.MAX_DIFF_LINES + 3)

    def test_the_same_drift_gives_the_same_hash(self):
        changed = json.loads(json.dumps(DOC))
        changed["components"]["schemas"]["Id"]["type"] = "integer"
        a, b = Path(self.dir.name) / "a.md", Path(self.dir.name) / "b.md"
        self.run_check(changed, "--issue-body", str(a))
        self.run_check(changed, "--issue-body", str(b))
        self.assertEqual(a.read_text(encoding="utf-8"), b.read_text(encoding="utf-8"))


class FetchTests(unittest.TestCase):
    class Resp:
        def __init__(self, body, status=200):
            self.body, self.status = body, status

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

        def read(self, n):
            return self.body[:n]

    def fetch_with(self, responses):
        queue = list(responses)

        class Opener:
            def open(self_, req, timeout):
                self.assertEqual(timeout, tool.TIMEOUT_SECS)
                self.assertNotIn("Authorization", {k.title() for k in req.headers})
                item = queue.pop(0)
                if isinstance(item, Exception):
                    raise item
                return item

        with mock.patch.object(tool.urllib.request, "build_opener", lambda *a: Opener()):
            return tool.fetch_openapi("https://fluxer.example")

    def test_discovery_then_document(self):
        disc = json.dumps({"endpoints": {"api_public": "https://api.example/x"}}).encode()
        doc = self.fetch_with([self.Resp(disc), self.Resp(b'{"paths": {}}')])
        self.assertEqual(doc, {"paths": {}})

    def test_failures_are_fetch_errors(self):
        good = self.Resp(json.dumps({"endpoints": {"api_public": "https://a.example"}}).encode())
        cases = [
            [self.Resp(b"not json")],
            [self.Resp(b"{}", status=503)],
            [self.Resp(b'{"endpoints": {"api_public": "http://plain.example"}}')],
            [self.Resp(b'{"endpoints": {}}')],
            [good, self.Resp(b"<html>")],
            [good, self.Resp(b"x" * (tool.MAX_BYTES + 1))],
            [good, tool.urllib.error.URLError("down")],
            [good, tool.urllib.error.HTTPError("u", 502, "bad", {}, None)],
            [good, TimeoutError()],
        ]
        for case in cases:
            with self.subTest(case=case):
                with self.assertRaises(tool.FetchError):
                    self.fetch_with(case)

    def test_only_https_addresses_are_fetched(self):
        with self.assertRaises(tool.FetchError):
            tool.http_get_json("http://fluxer.example/x")

    def test_redirects_to_plain_http_are_refused(self):
        handler = tool._HttpsOnlyRedirects()
        with self.assertRaises(tool.FetchError):
            handler.redirect_request(mock.Mock(), None, 302, "Found", {}, "http://evil.example/")


if __name__ == "__main__":
    unittest.main()
