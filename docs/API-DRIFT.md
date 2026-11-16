# API drift guard

Inlet talks to Fluxer's REST API. Fluxer changes its API without notice, so a weekly workflow (`.github/workflows/api-drift.yml`) compares the public OpenAPI document of each instance (`/v1/openapi.json`, found through the instance's discovery document) with a committed snapshot of the part Inlet depends on. When the two differ, it opens an issue titled `API drift: <instance>` with the differences in a collapsed block. While that issue is open, a later run only adds a comment if the differences changed.

The job needs no secret and sends no credential. The document is treated as untrusted input: the download has a size cap (16 MiB) and a 30 second timeout, only https addresses are followed, the text is parsed as JSON and never evaluated, and it reaches the issue only through a file. If the fetch fails (network error, HTTP error, not JSON, too large), the run fails and no issue is opened.

## What the snapshot holds

`tests/api-snapshot/<instance>.json` has two parts, both with sorted keys so a change shows as a small diff:

- `routes`: every route (method and path) that `inlet-protocol` calls, with the documented parameters, the request body schema and the 2xx response schemas. Prose (descriptions, titles, examples) and the error responses, which are the same on every route, are left out. A route the instance does not document is recorded as `{"missing": true}`.
- `schemas`: the schemas those routes refer to, directly or through other schemas, one entry each. References stay as names (`{"$ref": "UserPartialResponse"}`).

The list of routes is not written by hand. `tests/api-snapshot/api_snapshot.py` reads the production code of `crates/inlet-protocol/src` and collects every path literal together with the HTTP method used at its call site. Parameter names and query strings are ignored: `/channels/{}/messages` stands for any `/channels/{id}/messages?limit=..`. A path that another function extends (the reaction routes build on one base path) has no method at its own call site; those routes are listed in `tests/api-snapshot/routes-extra.txt`. The tool refuses to run when a path in the code has neither a method nor an entry there, and when an entry there matches no path in the code, so the list cannot fall behind the code unnoticed.

## Refresh the snapshot after an API change

1. Read the diff in the issue. Update `inlet-protocol` (and the app) for what changed.
2. If you added or removed a route, the tool tells you what is missing: run

   ```sh
   python3 tests/api-snapshot/api_snapshot.py verify
   ```

   A new route that cannot be resolved from its call site goes into `routes-extra.txt`.
3. Rewrite the snapshots from the live documents (this fetches the two public documents, nothing else; pass an instance name to refresh only one):

   ```sh
   python3 tests/api-snapshot/api_snapshot.py update
   ```

4. Check what changed with `git diff tests/api-snapshot`, run the tool's tests (`python3 -I -m unittest discover -s tests/api-snapshot`) and commit the snapshots together with the code change.
5. Close the issue once the commit is merged. The next run compares against the new snapshot.

Other commands: `routes` prints the routes Inlet uses, and `check <instance>` does what the workflow does (exit code 0 no drift, 1 drift, 2 stale route list or broken tool, 3 failed fetch). Both `update` and `check` accept `--from-file <document.json>` to read a saved document instead of fetching. The tool needs Python 3.9 or newer and nothing else.

To run the workflow by hand, use "Run workflow" on the Actions page.
