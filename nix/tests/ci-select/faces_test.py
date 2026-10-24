#!/usr/bin/env python3
# The face readers on small made-up trees: a change inside a handler, a
# comment or a test leaves a face as it was; a new route, a new JSON key,
# a new status, a changed flag or a wire struct's field changes it. With
# apidiff and goface on PATH (the ci-select check), the Go faces too.
import os
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import interfaces  # noqa: E402

GATE = {
    "lib.rs": """
use axum::{routing::get, Router, Json};
pub fn app() -> Router {
    Router::new()
        .route("/api/joke", get(joke))
        .route("/api/who", get(who).post(set_who))
}
async fn joke() -> Json<serde_json::Value> {
    let n = pick();
    Json(serde_json::json!({ "joke": n }))
}
fn pick() -> u32 { 4 }
async fn who() -> Json<Who> { Json(Who { name: "x".into() }) }
async fn set_who() -> StatusCode { StatusCode::NO_CONTENT }
#[derive(serde::Serialize)]
pub struct Who { pub name: String }
#[cfg(test)]
mod tests {
    #[test]
    fn t() { assert_eq!(super::pick(), 4); }
}
""",
}

CONTROL = {
    "go.mod": "module example.com/ctl\n\ngo 1.22\n",
    "go.sum": "",
    "state/state.go": """package state

// State is what every box agrees on.
type State struct {
	Boxes []string `json:"boxes"`
}

func Count(s *State) int { return len(s.Boxes) }
""",
    "cmd/ctl/main.go": """package main

import (
	"flag"
	"net/http"
)

func main() {
	listen := flag.String("listen", "127.0.0.1:7000", "where")
	flag.Parse()
	http.HandleFunc("/state", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	})
	http.ListenAndServe(*listen, nil)
}
""",
}


def tree(files, edits=None):
    d = tempfile.mkdtemp(prefix="faces-test.")
    for name, text in files.items():
        for old, new in (edits or {}).get(name, []):
            assert old in text, (name, old)
            text = text.replace(old, new)
        p = os.path.join(d, name)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "w").write(text)
    return d


def gate_face(t):
    return interfaces.face_text("rust-routes", t, ".")


def control_differs(a, b):
    out = []
    if interfaces.face_text("goface", a, ".") != interfaces.face_text("goface", b, "."):
        out.append("goface")
    # module mode with nothing required: no network, no vendor
    return out + interfaces.apidiff(a, b, ".", ["state"], None)


def main():
    bad = []
    base = tree(GATE)
    gate = [
        (
            "a handler's insides",
            {"lib.rs": [("fn pick() -> u32 { 4 }", "fn pick() -> u32 { 2 + 2 }")]},
            False,
        ),
        (
            "a comment",
            {"lib.rs": [("let n = pick();", "let n = pick(); // one")]},
            False,
        ),
        (
            "a test",
            {
                "lib.rs": [
                    ("assert_eq!(super::pick(), 4);", "assert!(super::pick() > 0);")
                ]
            },
            False,
        ),
        (
            "a new route",
            {
                "lib.rs": [
                    (
                        '.route("/api/joke", get(joke))',
                        '.route("/api/joke", get(joke))\n        .route("/api/new", get(joke))',
                    )
                ]
            },
            True,
        ),
        ("a JSON key", {"lib.rs": [('"joke": n', '"joke": n, "at": 1')]}, True),
        (
            "a status",
            {"lib.rs": [("StatusCode::NO_CONTENT", "StatusCode::ACCEPTED")]},
            True,
        ),
        (
            "a wire struct's field",
            {"lib.rs": [("pub name: String }", "pub name: String, pub age: u8 }")]},
            True,
        ),
        (
            "an environment variable",
            {
                "lib.rs": [
                    (
                        "fn pick() -> u32 { 4 }",
                        'fn pick() -> u32 { std::env::var("JOKE_N").map(|_| 4).unwrap_or(4) }',
                    )
                ]
            },
            True,
        ),
    ]
    for title, edits, moves in gate:
        head = tree(GATE, edits)
        differs = gate_face(base) != gate_face(head)
        ok = differs == moves
        print(
            f"{'ok  ' if ok else 'FAIL'} gate, {title}: face {'changed' if differs else 'held'}"
        )
        if not ok:
            bad.append(f"gate: {title}")
        shutil.rmtree(head)
    shutil.rmtree(base)

    if shutil.which("goface") and shutil.which("apidiff") and shutil.which("go"):
        base = tree(CONTROL)
        control = [
            (
                "a function's insides",
                {
                    "state/state.go": [
                        ("return len(s.Boxes)", "n := len(s.Boxes)\n\treturn n")
                    ]
                },
                False,
            ),
            (
                "a new exported function",
                {
                    "state/state.go": [
                        (
                            "func Count",
                            "func Empty(s *State) bool { return Count(s) == 0 }\n\nfunc Count",
                        )
                    ]
                },
                True,
            ),
            (
                "a JSON tag",
                {"state/state.go": [('`json:"boxes"`', '`json:"boxes,omitempty"`')]},
                True,
            ),
            (
                "a flag's default",
                {"cmd/ctl/main.go": [('"127.0.0.1:7000"', '"127.0.0.1:7001"')]},
                True,
            ),
            (
                "a new route",
                {
                    "cmd/ctl/main.go": [
                        (
                            'http.HandleFunc("/state"',
                            'http.HandleFunc("/plan", nil)\n\thttp.HandleFunc("/state"',
                        )
                    ]
                },
                True,
            ),
        ]
        os.environ.setdefault("GOCACHE", tempfile.mkdtemp(prefix="gocache."))
        os.environ.setdefault("HOME", tempfile.mkdtemp(prefix="home."))
        os.environ["GOPROXY"] = "off"
        for title, edits, moves in control:
            head = tree(CONTROL, edits)
            why = control_differs(base, head)
            ok = bool(why) == moves
            print(
                f"{'ok  ' if ok else 'FAIL'} control, {title}: {'; '.join(why) or 'face held'}"
            )
            if not ok:
                bad.append(f"control: {title}")
            shutil.rmtree(head)
        shutil.rmtree(base)
    else:
        print("goface, apidiff or go not on PATH: the Go faces are not tried")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
