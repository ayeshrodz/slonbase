// Go's html/template language, for PocketBase's `$template`: actions with pipelines,
// fields and methods, variables, if/else, range, with, define/template/block, break and
// continue, trim markers and comments, the built-in functions plus `raw`, and functions
// added with `$template.addFuncs()`.
//
// Output is HTML-escaped, except values from `raw` (and `html`). Unlike Go, escaping
// doesn't depend on the context: inside <script> or URLs, use the `js` and `urlquery`
// functions.
(function (g) {
  "use strict";

  class SafeHTML {
    constructor(s) {
      this.s = String(s);
    }
    toString() {
      return this.s;
    }
  }

  const ESCAPES = { "\0": "�", '"': "&#34;", "&": "&amp;", "'": "&#39;", "+": "&#43;", "<": "&lt;", ">": "&gt;" };
  const escapeHTML = (s) => String(s).replace(/[\0"&'+<>]/g, (c) => ESCAPES[c]);

  class TemplateError extends Error {}

  // the UTF-8 bytes of a string (Go's strings are bytes)
  const utf8 = (s) => Array.from(unescape(encodeURIComponent(s)), (c) => c.charCodeAt(0));

  // ---------------------------------------------------------------- lexer

  // splits the source into text and actions, applying trim markers and dropping comments
  function scan(name, src) {
    const parts = [];
    let pos = 0;
    let line = 1;
    let trimNext = false;
    while (pos < src.length) {
      const open = src.indexOf("{{", pos);
      let text = open < 0 ? src.slice(pos) : src.slice(pos, open);
      if (trimNext) text = text.replace(/^\s+/, "");
      trimNext = false;
      if (open < 0) {
        if (text) parts.push({ text });
        break;
      }
      let start = open + 2;
      if (src[start] === "-" && /\s/.test(src[start + 1] || "")) {
        text = text.replace(/\s+$/, "");
        start += 2;
      }
      if (text) parts.push({ text });
      line += (src.slice(pos, open).match(/\n/g) || []).length;
      // the action ends at the first "}}" outside a string or comment
      let i = start;
      let quote = null;
      let inComment = false;
      for (; i < src.length; i++) {
        const c = src[i];
        if (inComment) {
          if (c === "*" && src[i + 1] === "/") {
            inComment = false;
            i++;
          }
        } else if (quote) {
          if (c === "\\" && quote !== "`") i++;
          else if (c === quote) quote = null;
        } else if (c === '"' || c === "`" || c === "'") {
          quote = c;
        } else if (c === "/" && src[i + 1] === "*") {
          inComment = true;
          i++;
        } else if (c === "}" && src[i + 1] === "}") {
          break;
        }
      }
      if (i >= src.length) throw new TemplateError(`template: ${name}:${line}: unclosed action`);
      let body = src.slice(start, i);
      if (/\s-$/.test(body)) {
        body = body.slice(0, -2);
        trimNext = true;
      }
      const trimmed = body.trim();
      if (!(trimmed.startsWith("/*") && trimmed.endsWith("*/"))) parts.push({ action: body, line });
      line += (body.match(/\n/g) || []).length;
      pos = i + 2;
    }
    return parts;
  }

  function tokenize(src, where) {
    const tokens = [];
    let i = 0;
    const fail = (msg) => {
      throw new TemplateError(`template: ${where}: ${msg}`);
    };
    while (i < src.length) {
      const c = src[i];
      if (/\s/.test(c)) {
        i++;
      } else if (c === '"') {
        let j = i + 1;
        while (j < src.length && src[j] !== '"') j += src[j] === "\\" ? 2 : 1;
        if (j >= src.length) fail("unterminated quoted string");
        tokens.push({ t: "lit", v: JSON.parse(src.slice(i, j + 1)) });
        i = j + 1;
      } else if (c === "`") {
        const j = src.indexOf("`", i + 1);
        if (j < 0) fail("unterminated raw quoted string");
        tokens.push({ t: "lit", v: src.slice(i + 1, j) });
        i = j + 1;
      } else if (c === "'") {
        const j = src.indexOf("'", i + 1);
        if (j < 0) fail("unterminated character constant");
        tokens.push({ t: "lit", v: JSON.parse('"' + src.slice(i + 1, j) + '"').codePointAt(0) });
        i = j + 1;
      } else if (c === "(" || c === ")" || c === "|" || c === ",") {
        tokens.push({ t: c });
        i++;
      } else if (c === ":" && src[i + 1] === "=") {
        tokens.push({ t: ":=" });
        i += 2;
      } else if (c === "=") {
        tokens.push({ t: "=" });
        i++;
      } else if (c === "." || c === "$") {
        // .a.b, ., $x.a, $
        const m = /^(\$[A-Za-z0-9_]*|)((?:\.[A-Za-z0-9_]+)*)(\.?)/.exec(src.slice(i));
        const variable = m[1];
        const fields = m[2] ? m[2].slice(1).split(".") : [];
        if (variable) tokens.push({ t: "var", name: variable, fields });
        else tokens.push({ t: "field", fields });
        i += m[0].length || 1;
      } else if (/[-+0-9]/.test(c)) {
        const m = /^[-+]?(0x[0-9a-fA-F_]+|[0-9_]*\.?[0-9_]+(?:[eE][-+]?[0-9]+)?)/.exec(src.slice(i));
        if (!m) fail(`bad number syntax: ${src.slice(i, i + 10)}`);
        tokens.push({ t: "lit", v: Number(m[0].replace(/_/g, "")) });
        i += m[0].length;
      } else if (/[A-Za-z_]/.test(c)) {
        const m = /^[A-Za-z_][A-Za-z0-9_]*/.exec(src.slice(i));
        const word = m[0];
        i += word.length;
        if (word === "true" || word === "false") tokens.push({ t: "lit", v: word === "true" });
        else if (word === "nil") tokens.push({ t: "lit", v: null });
        else tokens.push({ t: "ident", v: word });
      } else {
        fail(`unexpected ${JSON.stringify(c)} in command`);
      }
      // a field after a parenthesized pipeline: (...).field
      const last = tokens[tokens.length - 1];
      if (last && last.t === ")" && src[i] === ".") {
        const m = /^((?:\.[A-Za-z0-9_]+)+)/.exec(src.slice(i));
        if (m) {
          tokens.push({ t: "chain", fields: m[1].slice(1).split(".") });
          i += m[1].length;
        }
      }
    }
    return tokens;
  }

  // ---------------------------------------------------------------- parser

  const KEYWORDS = ["if", "else", "end", "range", "with", "define", "template", "block", "break", "continue"];

  function parse(name, src, funcs, set) {
    const parts = scan(name, src);
    let i = 0;
    const where = (part) => `${name}:${part ? part.line : "?"}`;

    function parsePipeline(tokens, part, allowDecl) {
      let pos = 0;
      const pipe = { decl: [], assign: false, cmds: [] };
      if (allowDecl) {
        // $x := ..., $i, $v := ..., $x = ...
        let j = 0;
        const names = [];
        while (tokens[j] && tokens[j].t === "var" && tokens[j].fields.length === 0) {
          names.push(tokens[j].name);
          j++;
          if (tokens[j] && tokens[j].t === ",") j++;
          else break;
        }
        if (names.length && tokens[j] && (tokens[j].t === ":=" || tokens[j].t === "=")) {
          pipe.decl = names;
          pipe.assign = tokens[j].t === "=";
          pos = j + 1;
        }
      }
      const parseOperand = () => {
        const tok = tokens[pos];
        if (!tok) return null;
        if (tok.t === "(") {
          pos++;
          const start = pos;
          let depth = 1;
          while (pos < tokens.length && depth > 0) {
            if (tokens[pos].t === "(") depth++;
            else if (tokens[pos].t === ")") depth--;
            pos++;
          }
          if (depth) throw new TemplateError(`template: ${where(part)}: unclosed left paren`);
          const inner = parsePipeline(tokens.slice(start, pos - 1), part, false);
          let fields = [];
          if (tokens[pos] && tokens[pos].t === "chain") fields = tokens[pos++].fields;
          return { k: "pipe", pipe: inner, fields };
        }
        if (tok.t === "lit") {
          pos++;
          return { k: "lit", v: tok.v };
        }
        if (tok.t === "field") {
          pos++;
          return { k: "field", fields: tok.fields };
        }
        if (tok.t === "var") {
          pos++;
          return { k: "var", name: tok.name, fields: tok.fields };
        }
        if (tok.t === "ident") {
          pos++;
          if (!(tok.v in funcs) && !(tok.v in BUILTINS)) {
            throw new TemplateError(`template: ${where(part)}: function "${tok.v}" not defined`);
          }
          return { k: "func", name: tok.v };
        }
        return null;
      };
      while (pos < tokens.length) {
        const cmd = [];
        while (pos < tokens.length && tokens[pos].t !== "|") {
          const op = parseOperand();
          if (!op) throw new TemplateError(`template: ${where(part)}: unexpected ${tokens[pos].t} in operand`);
          cmd.push(op);
        }
        if (!cmd.length) throw new TemplateError(`template: ${where(part)}: missing command`);
        pipe.cmds.push(cmd);
        if (tokens[pos] && tokens[pos].t === "|") pos++;
      }
      if (!pipe.cmds.length) throw new TemplateError(`template: ${where(part)}: missing value for command`);
      return pipe;
    }

    // parses nodes until one of the `stops` keywords; returns [nodes, stop, part]
    function parseList(stops) {
      const nodes = [];
      while (i < parts.length) {
        const part = parts[i++];
        if (part.text !== undefined) {
          nodes.push({ k: "text", text: part.text });
          continue;
        }
        const tokens = tokenize(part.action, where(part));
        const head = tokens[0] && tokens[0].t === "ident" && KEYWORDS.includes(tokens[0].v) ? tokens[0].v : null;
        if (head && stops.includes(head)) return [nodes, head, part, tokens];
        switch (head) {
          case "if":
          case "range":
          case "with": {
            nodes.push(parseControl(head, tokens, part));
            break;
          }
          case "define": {
            if (!tokens[1] || tokens[1].t !== "lit") throw new TemplateError(`template: ${where(part)}: define needs a name`);
            const [body] = parseList(["end"]);
            set[tokens[1].v] = body;
            break;
          }
          case "block": {
            if (!tokens[1] || tokens[1].t !== "lit") throw new TemplateError(`template: ${where(part)}: block needs a name`);
            const pipe = tokens.length > 2 ? parsePipeline(tokens.slice(2), part, false) : null;
            const [body] = parseList(["end"]);
            if (!(tokens[1].v in set)) set[tokens[1].v] = body;
            nodes.push({ k: "template", name: tokens[1].v, pipe, line: part.line });
            break;
          }
          case "template": {
            if (!tokens[1] || tokens[1].t !== "lit") throw new TemplateError(`template: ${where(part)}: template needs a name`);
            const pipe = tokens.length > 2 ? parsePipeline(tokens.slice(2), part, false) : null;
            nodes.push({ k: "template", name: tokens[1].v, pipe, line: part.line });
            break;
          }
          case "break":
          case "continue":
            nodes.push({ k: head });
            break;
          case "else":
          case "end":
            throw new TemplateError(`template: ${where(part)}: unexpected {{${head}}}`);
          default:
            nodes.push({ k: "action", pipe: parsePipeline(tokens, part, true), line: part.line });
        }
      }
      if (stops.length) throw new TemplateError(`template: ${name}: unexpected EOF`);
      return [nodes, null, null, null];
    }

    function parseControl(kind, tokens, part) {
      const node = { k: kind, pipe: parsePipeline(tokens.slice(1), part, true), list: [], else: null, line: part.line };
      const [list, stop, stopPart, stopTokens] = parseList(["else", "end"]);
      node.list = list;
      if (stop === "else") {
        if (stopTokens.length > 1 && stopTokens[1].t === "ident" && (stopTokens[1].v === "if" || stopTokens[1].v === "with")) {
          // {{else if ...}}: a nested control that ends with this one's {{end}}
          node.else = [parseControl(stopTokens[1].v, stopTokens.slice(1), stopPart)];
        } else {
          const [elseList] = parseList(["end"]);
          node.else = elseList;
        }
      }
      return node;
    }

    const [nodes] = parseList([]);
    return nodes;
  }

  // ---------------------------------------------------------------- execution

  const isPlainObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v) && (Object.getPrototypeOf(v) === Object.prototype || Object.getPrototypeOf(v) === null);

  function truth(v) {
    if (v === null || v === undefined || v === false || v === 0 || v === "") return false;
    if (Array.isArray(v)) return v.length > 0;
    if (isPlainObject(v)) return Object.keys(v).length > 0;
    return true;
  }

  // numbers as Go prints them: integers (int64 from JavaScript) as they are, others as
  // float64 with %g (exponents from 1e+06 and below 1e-04)
  function goNumber(v) {
    if (Number.isNaN(v)) return "NaN";
    if (!Number.isFinite(v)) return v > 0 ? "+Inf" : "-Inf";
    if (Number.isInteger(v) && Math.abs(v) <= 9223372036854775807) return BigInt(v).toString();
    const [mantissa, exp] = v.toExponential().split("e");
    const e = Number(exp);
    if (e < -4 || e >= 6) return mantissa + "e" + (e < 0 ? "-" : "+") + String(Math.abs(e)).padStart(2, "0");
    return String(v);
  }

  function goString(v) {
    if (v === null || v === undefined) return "";
    if (typeof v === "number") return goNumber(v);
    if (v instanceof SafeHTML) return v.s;
    if (Array.isArray(v)) return "[" + v.map(goString).join(" ") + "]";
    if (isPlainObject(v)) {
      return "map[" + Object.keys(v).sort().map((k) => k + ":" + goString(v[k])).join(" ") + "]";
    }
    return String(v);
  }

  function printf(format, ...args) {
    let n = 0;
    return String(format).replace(/%([-+ #0]*)(\d+)?(?:\.(\d+))?([%vsdqfxXtgec])/g, (m, flags, width, prec, verb) => {
      if (verb === "%") return "%";
      const a = args[n++];
      let out;
      switch (verb) {
        case "d":
          out = Number.isInteger(Number(a)) ? goNumber(Number(a)) : `%!d(float64=${goString(a)})`;
          break;
        case "f":
          out = Number(a).toFixed(prec === undefined ? 6 : Number(prec));
          break;
        case "g":
        case "e":
          out = String(Number(a));
          break;
        case "q":
          out = JSON.stringify(goString(a));
          break;
        case "x":
        case "X":
          out = typeof a === "number" ? a.toString(16) : utf8(goString(a)).map((b) => b.toString(16).padStart(2, "0")).join("");
          if (verb === "X") out = out.toUpperCase();
          break;
        case "t":
          out = String(!!a);
          break;
        case "c":
          out = String.fromCodePoint(Number(a));
          break;
        default:
          out = goString(a);
          if (prec !== undefined && verb === "s") out = out.slice(0, Number(prec));
      }
      if (width && out.length < Number(width)) {
        out = flags.includes("-") ? out.padEnd(Number(width)) : out.padStart(Number(width), flags.includes("0") && verb !== "s" ? "0" : " ");
      }
      return out;
    });
  }

  // fmt.Sprint: spaces between operands when neither is a string
  function sprint(args) {
    let out = "";
    args.forEach((a, i) => {
      if (i > 0 && typeof a !== "string" && typeof args[i - 1] !== "string") out += " ";
      out += goString(a);
    });
    return out;
  }

  function compare(a, b) {
    if (typeof a === "number" && typeof b === "number") return a - b;
    if (typeof a === "string" && typeof b === "string") return a < b ? -1 : a > b ? 1 : 0;
    throw new TemplateError("incompatible types for comparison");
  }

  const BUILTINS = {
    and: (...a) => {
      for (const v of a) if (!truth(v)) return v;
      return a[a.length - 1];
    },
    or: (...a) => {
      for (const v of a) if (truth(v)) return v;
      return a[a.length - 1];
    },
    not: (v) => !truth(v),
    len: (v) => {
      if (typeof v === "string") return utf8(v).length;
      if (Array.isArray(v)) return v.length;
      if (v && typeof v === "object") return Object.keys(v).length;
      throw new TemplateError("len of " + typeof v);
    },
    index: (v, ...keys) => {
      for (const k of keys) {
        if (v === null || v === undefined) throw new TemplateError("index of untyped nil");
        v = v[k];
      }
      return v === undefined ? null : v;
    },
    slice: (v, i, j) => (v === null || v === undefined ? v : v.slice(i || 0, j === undefined ? undefined : j)),
    eq: (a, ...b) => b.some((x) => a === x || (a == null && x == null)),
    ne: (a, b) => a !== b,
    lt: (a, b) => compare(a, b) < 0,
    le: (a, b) => compare(a, b) <= 0,
    gt: (a, b) => compare(a, b) > 0,
    ge: (a, b) => compare(a, b) >= 0,
    print: (...a) => sprint(a),
    println: (...a) => a.map(goString).join(" ") + "\n",
    printf: (f, ...a) => printf(f, ...a),
    html: (...a) => new SafeHTML(escapeHTML(sprint(a))),
    js: (...a) =>
      sprint(a).replace(/[\\'"<>&=\n\r\t]/g, (c) =>
        ({ "\\": "\\\\", "'": "\\'", '"': "\\\"", "<": "\\u003C", ">": "\\u003E", "&": "\\u0026", "=": "\\u003D", "\n": "\\n", "\r": "\\r", "\t": "\\t" })[c],
      ),
    urlquery: (...a) => encodeURIComponent(sprint(a)).replace(/%20/g, "+").replace(/[!'()*]/g, (c) => "%" + c.charCodeAt(0).toString(16).toUpperCase()),
    raw: (s) => new SafeHTML(s === null || s === undefined ? "" : s),
    call: (fn, ...a) => {
      if (typeof fn !== "function") throw new TemplateError("call of non-function");
      return fn(...a);
    },
  };

  class Break {}
  class Continue {}

  class Exec {
    constructor(set, funcs, name) {
      this.set = set;
      this.funcs = funcs;
      this.name = name;
      this.out = [];
      this.depth = 0;
    }

    fail(node, msg) {
      throw new TemplateError(`template: ${this.name}:${node && node.line ? node.line : "?"}: ${msg}`);
    }

    // follows fields from a value; a method is called (the last one with `args`)
    fields(value, names, args, node) {
      let v = value;
      for (let n = 0; n < names.length; n++) {
        const name = names[n];
        const last = n === names.length - 1;
        if (v === null || v === undefined) this.fail(node, `nil pointer evaluating interface {}.${name}`);
        const owner = v;
        v = owner[name];
        if (typeof v === "function") {
          v = v.apply(owner, last ? args : []);
          if (last) return v === undefined ? null : v;
        } else if (last && args.length) {
          this.fail(node, `${name} is not a method but has arguments`);
        }
      }
      return v === undefined ? null : v;
    }

    command(cmd, dot, vars, piped, node) {
      const first = cmd[0];
      const rest = () => {
        const args = cmd.slice(1).map((op) => this.operand(op, dot, vars, node));
        if (piped !== undefined) args.push(piped);
        return args;
      };
      switch (first.k) {
        case "func": {
          const fn = this.funcs[first.name] || BUILTINS[first.name];
          // and/or evaluate lazily in Go; the values are already computed here
          return fn(...rest());
        }
        case "field":
          return this.fields(dot, first.fields, rest(), node);
        case "var": {
          const base = this.lookup(vars, first.name, node);
          return first.fields.length ? this.fields(base, first.fields, rest(), node) : this.noArgs(cmd, piped, node, base);
        }
        case "pipe": {
          const base = this.pipeline(first.pipe, dot, vars, node);
          return first.fields.length ? this.fields(base, first.fields, rest(), node) : this.noArgs(cmd, piped, node, base);
        }
        default:
          return this.noArgs(cmd, piped, node, first.v);
      }
    }

    noArgs(cmd, piped, node, value) {
      if (cmd.length > 1 || piped !== undefined) this.fail(node, "can't give argument to non-function");
      return value;
    }

    operand(op, dot, vars, node) {
      switch (op.k) {
        case "lit":
          return op.v;
        case "field":
          return this.fields(dot, op.fields, [], node);
        case "var": {
          const base = this.lookup(vars, op.name, node);
          return this.fields(base, op.fields, [], node);
        }
        case "pipe": {
          const base = this.pipeline(op.pipe, dot, vars, node);
          return this.fields(base, op.fields, [], node);
        }
        case "func":
          return (this.funcs[op.name] || BUILTINS[op.name])();
      }
      return null;
    }

    lookup(vars, name, node) {
      for (let s = vars.length - 1; s >= 0; s--) if (name in vars[s]) return vars[s][name];
      this.fail(node, `undefined variable "${name}"`);
    }

    pipeline(pipe, dot, vars, node) {
      let value;
      for (const cmd of pipe.cmds) value = this.command(cmd, dot, vars, value, node);
      if (pipe.decl.length) {
        if (pipe.assign) {
          const name = pipe.decl[0];
          let s = vars.length - 1;
          while (s >= 0 && !(name in vars[s])) s--;
          if (s < 0) this.fail(node, `undefined variable "${name}"`);
          vars[s][name] = value;
        } else {
          vars[vars.length - 1][pipe.decl[pipe.decl.length - 1]] = value;
        }
      }
      return value === undefined ? null : value;
    }

    list(nodes, dot, vars) {
      for (const node of nodes) this.node(node, dot, vars);
    }

    node(node, dot, vars) {
      switch (node.k) {
        case "text":
          this.out.push(node.text);
          return;
        case "action": {
          const v = this.pipeline(node.pipe, dot, vars, node);
          if (!node.pipe.decl.length) this.out.push(v instanceof SafeHTML ? v.s : escapeHTML(goString(v)));
          return;
        }
        case "if":
        case "with": {
          const scope = vars.concat([{}]);
          const v = this.pipeline(node.pipe, dot, scope, node);
          if (truth(v)) this.list(node.list, node.k === "with" ? v : dot, scope);
          else if (node.else) this.list(node.else, dot, scope);
          return;
        }
        case "range":
          return this.range(node, dot, vars);
        case "template": {
          const body = this.set[node.name];
          if (!body) this.fail(node, `no such template "${node.name}"`);
          if (++this.depth > 100000) this.fail(node, "exceeded maximum template depth");
          const data = node.pipe ? this.pipeline(node.pipe, dot, vars.concat([{}]), node) : null;
          this.list(body, data, [{ $: data }]);
          this.depth--;
          return;
        }
        case "break":
          throw new Break();
        case "continue":
          throw new Continue();
      }
    }

    range(node, dot, vars) {
      const scope = vars.concat([{}]);
      // evaluate without declaring: the variables are the key and the element
      const decl = node.pipe.decl;
      const v = this.pipeline({ decl: [], assign: false, cmds: node.pipe.cmds }, dot, scope, node);
      let entries;
      if (Array.isArray(v)) entries = v.map((x, i) => [i, x]);
      else if (typeof v === "number") entries = Array.from({ length: Math.max(0, Math.trunc(v)) }, (_, i) => [i, i]);
      else if (v && typeof v === "object") entries = Object.keys(v).sort().map((k) => [k, v[k]]);
      else if (v === null || v === undefined) entries = [];
      else this.fail(node, `range can't iterate over ${goString(v)}`);
      if (!entries.length) {
        if (node.else) this.list(node.else, dot, scope);
        return;
      }
      for (const [key, elem] of entries) {
        const iter = scope.concat([{}]);
        if (decl.length === 1) iter[iter.length - 1][decl[0]] = elem;
        if (decl.length === 2) {
          iter[iter.length - 1][decl[0]] = key;
          iter[iter.length - 1][decl[1]] = elem;
        }
        try {
          this.list(node.list, elem, iter);
        } catch (err) {
          if (err instanceof Break) break;
          if (!(err instanceof Continue)) throw err;
        }
      }
    }
  }

  // ---------------------------------------------------------------- the registry

  class Renderer {
    constructor(build) {
      try {
        Object.assign(this, build());
      } catch (err) {
        this.error = err;
      }
    }
    render(data) {
      if (this.error) throw this.error;
      const exec = new Exec(this.set, this.funcs, this.name);
      exec.list(this.set[this.name] || [], data === undefined ? null : data, [{ $: data === undefined ? null : data }]);
      return exec.out.join("");
    }
  }

  const basename = (path) => String(path).split(/[\\/]/).pop();

  // `readFile(path)` returns a file's text
  g.__newTemplateRegistry = (readFile) => {
    const funcs = Object.create(null);
    const cache = new Map();
    const cached = (key, build) => {
      if (!cache.has(key)) cache.set(key, new Renderer(build));
      return cache.get(key);
    };
    return {
      // {name: fn}: functions templates can call (like `raw`)
      addFuncs(map) {
        for (const name of Object.keys(map || {})) funcs[name] = map[name];
        cache.clear();
        return this;
      },
      // the first file is rendered; the others usually `define` the blocks it uses
      loadFiles(...files) {
        return cached("files:" + files.join(","), () => {
          const set = Object.create(null);
          for (const file of files) set[basename(file)] = parse(basename(file), readFile(file), funcs, set);
          return { set, funcs, name: basename(files[0]) };
        });
      },
      loadString(text) {
        return cached("string:" + text, () => {
          const set = Object.create(null);
          set[""] = parse("", String(text), funcs, set);
          return { set, funcs, name: "" };
        });
      },
    };
  };
})(globalThis);
