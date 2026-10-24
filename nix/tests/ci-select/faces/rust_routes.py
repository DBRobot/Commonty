#!/usr/bin/env python3
# An axum service's outward face, read from its source: every route (method,
# path, handler), the request and response types its handler declares and
# the crate's own definitions of them, and what its body (and the crate's
# functions it calls) reads and answers - JSON keys, status codes, header
# names, redirects. Plus the environment variables the crate reads and the
# box paths it names. Test code (#[cfg(test)]) is left out.
#
#   rust_routes.py SRC_DIR > routes.json
#
# A change that leaves this file byte for byte the same changed the
# service's insides alone (docs/dev/ci.md, Interfaces).
import json
import os
import re
import sys

KEEP = re.compile(r"\s+")


def lex_strip(text):
    """the text with comments blanked; strings kept as written"""
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            out.append(" ")
            i = j
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out.append(" ")
            i = j
        elif (
            c == "r"
            and re.match(r'r#*"', text[i:])
            and not re.match(r"\w", text[i - 1 : i] or " ")
        ):
            m = re.match(r'r(#*)"', text[i:])
            end = '"' + m.group(1)
            j = text.find(end, i + len(m.group(0)))
            j = n if j < 0 else j + len(end)
            out.append(text[i:j])
            i = j
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            out.append(text[i : j + 1])
            i = j + 1
        elif c == "'":
            # a char literal, or a lifetime
            m = re.match(r"'(\\.[^']*|[^\\'])'", text[i:])
            if m:
                out.append(m.group(0))
                i += len(m.group(0))
            else:
                out.append(c)
                i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


def skip_string(text, i):
    """index past the string or char literal at i, or i when none starts there"""
    if text[i] == '"':
        j = i + 1
        while j < len(text) and text[j] != '"':
            j += 2 if text[j] == "\\" else 1
        return j + 1
    m = re.match(r'r(#*)"', text[i:])
    if m and not re.match(r"\w", text[i - 1 : i] or " "):
        end = '"' + m.group(1)
        j = text.find(end, i + len(m.group(0)))
        return len(text) if j < 0 else j + len(end)
    m = re.match(r"'(\\.[^']*|[^\\'])'", text[i:])
    if m:
        return i + len(m.group(0))
    return i


def match_close(text, i):
    """i at an opening bracket: the index of its closing one"""
    pairs = {"(": ")", "[": "]", "{": "}"}
    stack = [pairs[text[i]]]
    j = i + 1
    while j < len(text) and stack:
        k = skip_string(text, j)
        if k != j:
            j = k
            continue
        c = text[j]
        if c in pairs:
            stack.append(pairs[c])
        elif c in ")]}":
            stack.pop()
        j += 1
    return j - 1


def drop_test_items(text):
    """the text without #[cfg(test)] items (a mod, a fn, an impl)"""
    while True:
        m = re.search(r"#\[cfg\(test\)\]", text)
        if not m:
            return text
        j = m.end()
        # the item's own brace block, or its semicolon
        k = j
        while k < len(text) and text[k] not in "{;":
            s = skip_string(text, k)
            k = s if s != k else k + 1
        if k < len(text) and text[k] == "{":
            k = match_close(text, k)
        text = text[: m.start()] + text[k + 1 :]


def norm(s):
    return KEEP.sub(" ", s).strip()


def split_args(s):
    """top-level comma split of an argument list's inside"""
    out, depth, cur, i = [], 0, [], 0
    while i < len(s):
        k = skip_string(s, i)
        if k != i:
            cur.append(s[i:k])
            i = k
            continue
        c = s[i]
        if c in "([{<":
            depth += 1
        elif c in ")]}>":
            depth -= 1
        if c == "," and depth == 0:
            out.append("".join(cur))
            cur = []
        else:
            cur.append(c)
        i += 1
    if "".join(cur).strip():
        out.append("".join(cur))
    return [x.strip() for x in out]


class Crate:
    def __init__(self, src):
        self.mods = {}
        for root, dirs, names in os.walk(src):
            dirs.sort()
            for f in sorted(names):
                if not f.endswith(".rs"):
                    continue
                path = os.path.join(root, f)
                rel = os.path.relpath(path, src)[:-3]
                mod = (
                    ""
                    if rel == "lib"
                    else rel.replace(os.sep, "::").removesuffix("::mod")
                )
                self.mods[mod] = drop_test_items(
                    lex_strip(open(path, encoding="utf-8").read())
                )
        self.text = "\n".join(self.mods[m] for m in sorted(self.mods))
        self.fns, self.byname = {}, {}
        for mod, text in sorted(self.mods.items()):
            for m in re.finditer(r"\bfn\s+(\w+)\s*(?:<[^{;]*?>)?\s*\(", text):
                close = match_close(text, m.end() - 1)
                j = close + 1
                while j < len(text) and text[j] not in "{;":
                    j += 1
                if j >= len(text) or text[j] == ";":
                    continue
                body_end = match_close(text, j)
                self.fns.setdefault((mod, m.group(1)), []).append(
                    (norm(text[m.start() : j]), text[j : body_end + 1])
                )
                self.byname.setdefault(m.group(1), set()).add(mod)
        self.types = {}
        for m in re.finditer(
            r"((?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?(struct|enum|type)\s+(\w+)",
            self.text,
        ):
            j = m.end()
            while j < len(self.text) and self.text[j] not in "{;":
                j += 1
            end = (
                match_close(self.text, j)
                if j < len(self.text) and self.text[j] == "{"
                else j
            )
            attrs = " ".join(
                a
                for a in re.findall(r"#\[[^\]]*\]", m.group(1))
                if "serde" in a or "derive" in a
            )
            self.types.setdefault(m.group(3), set()).add(
                norm(attrs + " " + self.text[m.start(2) : end + 1])
            )

    def resolve(self, mod, path):
        """the crate function a call or handler path names, seen from mod"""
        parts = [p for p in path.split("::") if p]
        name = parts[-1]
        if len(parts) > 1:
            # the nearest file module on the path (an inline mod lives in its file's)
            for q in reversed(parts[:-1]):
                m = "" if q in ("crate", "super") else (mod if q == "self" else q)
                if (m, name) in self.fns:
                    return [(m, name)]
            return []
        if (mod, name) in self.fns:
            return [(mod, name)]
        mods = self.byname.get(name, set())
        return [(next(iter(mods)), name)] if len(mods) == 1 else []

    def type_defs(self, text, seen=None):
        """the crate's definitions of every type named in text, followed through their fields"""
        seen = {} if seen is None else seen
        for name in re.findall(r"\b([A-Z]\w*)\b", text):
            if name in seen or name not in self.types:
                continue
            seen[name] = sorted(self.types[name])
            for d in seen[name]:
                self.type_defs(d, seen)
        return seen

    def reads(self, mod, body, depth=3, seen=None):
        """what a handler's body and the crate functions it calls read and answer"""
        seen = set() if seen is None else seen
        out = {
            "keys": set(),
            "status": set(),
            "headers": set(),
            "redirects": set(),
            "types": set(),
        }
        for m in re.finditer(r"json!\s*\(", body):
            inner = body[m.end() - 1 : match_close(body, m.end() - 1) + 1]
            out["keys"] |= set(re.findall(r'"([^"\\]+)"\s*:', inner))
        out["keys"] |= set(re.findall(r'\[\s*"([^"\\]+)"\s*\]', body))
        out["keys"] |= set(re.findall(r'\.get(?:_mut)?\(\s*"([^"\\]+)"\s*\)', body))
        out["status"] |= set(re.findall(r"StatusCode::([A-Z_]+)", body))
        out["headers"] |= set(re.findall(r"header::([A-Z_]+)", body))
        out["headers"] |= set(
            h.lower()
            for h in re.findall(r'"((?:[Xx]-|[A-Za-z]+-)[A-Za-z-]+)"\s*[,)]', body)
            if "-" in h
        )
        out["redirects"] |= set(re.findall(r'Redirect::\w+\(\s*"([^"]+)"', body))
        out["types"] |= set(
            re.findall(r"\b(?:Json|Query|Form|Path)\s*::<\s*([A-Z]\w*)", body)
        )
        if depth:
            for path in sorted(
                set(
                    re.findall(
                        r"(?<![.\w:])((?:\w+::)*\w+)\s*(?:::<[^>]*>)?\s*\(", body
                    )
                )
            ):
                for key in self.resolve(mod, path):
                    if key in seen:
                        continue
                    seen.add(key)
                    for _, b in self.fns[key]:
                        sub = self.reads(key[0], b, depth - 1, seen)
                        for k in out:
                            out[k] |= sub[k]
        return out


def wire_types(sig):
    """the types a signature takes off the wire or answers with: inside its
    Json, Query, Form and Path extractors, and its return type - never the
    service's own state (State, Extension)"""
    out = []
    for m in re.finditer(r"\b(?:Json|Query|Form|Path|TypedHeader)\s*<", sig):
        i = m.end() - 1
        depth, j = 0, i
        while j < len(sig):
            if sig[j] == "<":
                depth += 1
            elif sig[j] == ">":
                depth -= 1
                if not depth:
                    break
            j += 1
        out.append(sig[i + 1 : j])
    if "->" in sig:
        out.append(sig.rsplit("->", 1)[1])
    return " ".join(out)


METHODS = r"(get|post|put|delete|patch|head|options|any|trace)"


def routes(c):
    out = {}
    for mod, text in sorted(c.mods.items()):
        for m in re.finditer(r"\.route\s*\(", text):
            close = match_close(text, m.end() - 1)
            args = split_args(text[m.end() : close])
            if len(args) != 2 or not args[0].startswith('"'):
                continue
            path = args[0].strip('"')
            expr = args[1]
            for mm in re.finditer(
                r"(?<![\w.])(?:axum::routing::)?"
                + METHODS
                + r"\s*\(|\.\s*"
                + METHODS
                + r"\s*\(",
                expr,
            ):
                method = (mm.group(1) or mm.group(2)).upper()
                inner_open = mm.end() - 1
                inner = expr[inner_open + 1 : match_close(expr, inner_open)].strip()
                item = {}
                if re.fullmatch(r"[\w:]+(?:::<[^>]*>)?", inner):
                    keys = c.resolve(mod, re.sub(r"::<.*", "", inner))
                    fns = [f for k in keys for f in c.fns[k]]
                    item["handler"] = (
                        "::".join(x for x in keys[0] if x) if keys else inner
                    )
                    if not keys:
                        # not found: whatever it is, it counts as written
                        item["unresolved"] = norm(inner)
                    item["signature"] = sorted(sig for sig, _ in fns)
                    r = c.reads(
                        keys[0][0] if keys else mod, " ".join(b for _, b in fns)
                    )
                    sigs = " ".join(sig for sig, _ in fns)
                else:
                    # a closure: its parameters are its signature, the rest its body
                    pm = re.match(r"(?:move\s*)?\|([^|]*)\|", inner)
                    item["handler"] = "closure"
                    item["signature"] = [norm(pm.group(1))] if pm else []
                    sigs = pm.group(1) if pm else ""
                    r = c.reads(mod, inner)
                item["schemas"] = c.type_defs(
                    wire_types(sigs) + " " + " ".join(sorted(r["types"]))
                )
                for k in ("keys", "status", "headers", "redirects"):
                    item[k] = sorted(r[k])
                out.setdefault(f"{method} {path}", []).append(item)
    return {
        k: sorted(v, key=lambda x: json.dumps(x, sort_keys=True))
        for k, v in sorted(out.items())
    }


def main():
    src = sys.argv[1]
    c = Crate(src)
    face = {
        "routes": routes(c),
        "env": sorted(
            set(
                re.findall(
                    r'(?:env::var(?:_os)?|option_env!|env!)\(\s*"([A-Z0-9_]+)"', c.text
                )
            )
        ),
        "files": sorted(set(re.findall(r'"(/(?:var/lib|run|etc)/[^"\s{}]+)"', c.text))),
        "ports": sorted(
            set(
                re.findall(
                    r'"(?:127\.0\.0\.1|0\.0\.0\.0|\[::1?\]|localhost):(\d{2,5})"',
                    c.text,
                )
            )
        ),
    }
    json.dump(face, sys.stdout, indent=1, sort_keys=True)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
