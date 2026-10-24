// A Go program's outward face, read from its source with go/ast: the HTTP
// routes it registers (and what each handler answers: status codes, map
// keys, headers, the JSON types it builds), its flags, the environment it
// reads, every struct that goes over the wire as JSON (its fields and tags),
// and the box paths and ports it names. Tests, vendor and third_party are
// left out.
//
//	goface DIR > face.json
//
// A change that leaves this file byte for byte the same changed the
// program's insides alone (docs/dev/ci.md, Interfaces).
package main

import (
	"bytes"
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/printer"
	"go/token"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

type route struct {
	Handler string   `json:"handler"`
	Status  []string `json:"status"`
	Keys    []string `json:"keys"`
	Headers []string `json:"headers"`
	Types   []string `json:"types"`
}

type flagDef struct {
	Kind    string `json:"kind"`
	Default string `json:"default"`
}

type face struct {
	Routes map[string][]route   `json:"routes"`
	Flags  map[string][]flagDef `json:"flags"`
	Env    []string             `json:"env"`
	Wire   map[string]string    `json:"wire"`
	Files  []string             `json:"files"`
	Ports  []string             `json:"ports"`
}

var (
	fset    = token.NewFileSet()
	boxPath = regexp.MustCompile(`^/(var/lib|run|etc)/\S+$`)
	port    = regexp.MustCompile(`^(127\.0\.0\.1|0\.0\.0\.0|localhost|\[::1?\])?:(\d{2,5})$`)
)

func src(n ast.Node) string {
	var b bytes.Buffer
	printer.Fprint(&b, fset, n)
	return strings.Join(strings.Fields(b.String()), " ")
}

func lit(e ast.Expr) (string, bool) {
	if b, ok := e.(*ast.BasicLit); ok && b.Kind == token.STRING {
		s, err := strconv.Unquote(b.Value)
		return s, err == nil
	}
	return "", false
}

func set(m map[string]bool) []string {
	out := []string{}
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func main() {
	root := os.Args[1]
	f := face{Routes: map[string][]route{}, Flags: map[string][]flagDef{}, Wire: map[string]string{}}
	env, files, ports := map[string]bool{}, map[string]bool{}, map[string]bool{}
	type pkgFile struct {
		pkg  string
		file *ast.File
	}
	var all []pkgFile
	filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() && (d.Name() == "third_party" || d.Name() == "vendor" || d.Name() == "testdata") {
			return filepath.SkipDir
		}
		if d.IsDir() || !strings.HasSuffix(p, ".go") || strings.HasSuffix(p, "_test.go") {
			return nil
		}
		file, err := parser.ParseFile(fset, p, nil, 0)
		if err != nil {
			panic(err)
		}
		rel, _ := filepath.Rel(root, filepath.Dir(p))
		all = append(all, pkgFile{filepath.ToSlash(rel), file})
		return nil
	})
	sort.Slice(all, func(i, j int) bool {
		return fset.Position(all[i].file.Pos()).Filename < fset.Position(all[j].file.Pos()).Filename
	})
	// every function by package and name, for handlers named rather than written in place
	funcs := map[string]*ast.FuncDecl{}
	for _, pf := range all {
		for _, d := range pf.file.Decls {
			if fd, ok := d.(*ast.FuncDecl); ok && fd.Recv == nil {
				funcs[pf.pkg+"."+fd.Name.Name] = fd
			}
		}
	}
	answers := func(n ast.Node) route {
		st, keys, hdr, types := map[string]bool{}, map[string]bool{}, map[string]bool{}, map[string]bool{}
		ast.Inspect(n, func(x ast.Node) bool {
			switch v := x.(type) {
			case *ast.SelectorExpr:
				if id, ok := v.X.(*ast.Ident); ok && id.Name == "http" && strings.HasPrefix(v.Sel.Name, "Status") {
					st[v.Sel.Name] = true
				}
			case *ast.CompositeLit:
				if mt, ok := v.Type.(*ast.MapType); ok {
					if k, ok := mt.Key.(*ast.Ident); ok && k.Name == "string" {
						for _, e := range v.Elts {
							if kv, ok := e.(*ast.KeyValueExpr); ok {
								if s, ok := lit(kv.Key); ok {
									keys[s] = true
								}
							}
						}
					}
				} else if v.Type != nil {
					types[src(v.Type)] = true
				}
			case *ast.CallExpr:
				if sel, ok := v.Fun.(*ast.SelectorExpr); ok && (sel.Sel.Name == "Set" || sel.Sel.Name == "Add" || sel.Sel.Name == "Get") && len(v.Args) > 0 {
					if s, ok := lit(v.Args[0]); ok && strings.Contains(s, "-") || ok && strings.EqualFold(s, "Authorization") {
						hdr[strings.ToLower(s)] = true
					}
				}
			}
			return true
		})
		return route{Status: set(st), Keys: set(keys), Headers: set(hdr), Types: set(types)}
	}
	for _, pf := range all {
		ast.Inspect(pf.file, func(x ast.Node) bool {
			switch v := x.(type) {
			case *ast.CallExpr:
				sel, ok := v.Fun.(*ast.SelectorExpr)
				if !ok {
					return true
				}
				recv := ""
				if id, ok := sel.X.(*ast.Ident); ok {
					recv = id.Name
				}
				switch {
				case (sel.Sel.Name == "HandleFunc" || sel.Sel.Name == "Handle") && len(v.Args) == 2:
					pat, ok := lit(v.Args[0])
					if !ok {
						return true
					}
					r := route{}
					switch h := v.Args[1].(type) {
					case *ast.FuncLit:
						r = answers(h)
						r.Handler = "func"
					case *ast.Ident:
						if fd := funcs[pf.pkg+"."+h.Name]; fd != nil {
							r = answers(fd)
						}
						r.Handler = h.Name
					default:
						r = answers(h)
						r.Handler = src(h)
					}
					key := pf.pkg + " " + pat
					f.Routes[key] = append(f.Routes[key], r)
				case recv == "flag" && len(v.Args) >= 2:
					i := 0
					if strings.HasSuffix(sel.Sel.Name, "Var") {
						i = 1
					}
					if len(v.Args) > i+1 {
						if name, ok := lit(v.Args[i]); ok {
							key := pf.pkg + " -" + name
							f.Flags[key] = append(f.Flags[key], flagDef{Kind: sel.Sel.Name, Default: src(v.Args[i+1])})
						}
					}
				case recv == "os" && (sel.Sel.Name == "Getenv" || sel.Sel.Name == "LookupEnv") && len(v.Args) == 1:
					if s, ok := lit(v.Args[0]); ok {
						env[s] = true
					}
				}
			case *ast.BasicLit:
				if s, ok := lit(v); ok {
					if boxPath.MatchString(s) {
						files[s] = true
					}
					if m := port.FindStringSubmatch(s); m != nil {
						ports[m[2]] = true
					}
				}
			case *ast.TypeSpec:
				st, ok := v.Type.(*ast.StructType)
				if !ok {
					return true
				}
				tagged := false
				var fields []string
				for _, fl := range st.Fields.List {
					tag := ""
					if fl.Tag != nil {
						tag = fl.Tag.Value
						if strings.Contains(tag, "json:") {
							tagged = true
						}
					}
					names := []string{}
					for _, n := range fl.Names {
						names = append(names, n.Name)
					}
					fields = append(fields, strings.TrimSpace(strings.Join(names, ", ")+" "+src(fl.Type)+" "+tag))
				}
				if tagged {
					f.Wire[pf.pkg+"."+v.Name.Name] = "{ " + strings.Join(fields, "; ") + " }"
				}
			}
			return true
		})
	}
	f.Env, f.Files, f.Ports = set(env), set(files), set(ports)
	out, _ := json.MarshalIndent(f, "", " ")
	os.Stdout.Write(append(out, '\n'))
}
