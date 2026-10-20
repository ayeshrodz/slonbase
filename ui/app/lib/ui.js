// Shared components.
import {
  html,
  useState,
  useEffect,
  useRef,
  useLayoutEffect,
  createStore,
  useStore,
  cx,
  useClickOutside,
  render,
} from "./core.js";
import { Icon } from "./icons.js";

// ------------------------------------------------------------------ buttons

export function Button({ variant = "", size = "", icon, iconRight, loading, class: cls, children, ...rest }) {
  return html`<button
    type="button"
    class=${cx("btn", variant, size, { loading, "icon-only": icon && !children }, cls)}
    disabled=${rest.disabled || loading}
    ...${rest}
  >
    ${loading && html`<span class="spinner"></span>`}
    ${icon && html`<${Icon} name=${icon} size=${size === "xs" ? 14 : 16} />`}
    ${children && html`<span>${children}</span>`}
    ${iconRight && html`<${Icon} name=${iconRight} size=${16} />`}
  </button>`;
}

export function IconButton({ icon, label, size, class: cls, ...rest }) {
  return html`<button type="button" class=${cx("icon-btn", size, label && "tip", cls)} data-tip=${label} aria-label=${label} ...${rest}>
    <${Icon} name=${icon} size=${size === "sm" ? 15 : 17} />
  </button>`;
}

export function Spinner({ size = 16 }) {
  return html`<span class="spinner" style=${{ width: size + "px", height: size + "px" }}></span>`;
}

// ------------------------------------------------------------------ form controls

export function Field({ label, required, help, error, children, class: cls, extra }) {
  return html`<label class=${cx("field", cls)}>
    ${label &&
    html`<span class="label">${label}${required && html`<span class="req">*</span>`}<span class="spacer"></span>${extra}</span>`}
    ${children}
    ${error ? html`<span class="error">${error}</span>` : help && html`<span class="help">${help}</span>`}
  </label>`;
}

export function Input({ value, onInput, invalid, class: cls, ...rest }) {
  return html`<input
    class=${cx("input", { invalid }, cls)}
    value=${value ?? ""}
    onInput=${(e) => onInput && onInput(e.target.value, e)}
    ...${rest}
  />`;
}

export function TextArea({ value, onInput, invalid, code, class: cls, ...rest }) {
  return html`<textarea
    class=${cx("input", { invalid, code }, cls)}
    value=${value ?? ""}
    onInput=${(e) => onInput && onInput(e.target.value, e)}
    spellcheck=${code ? "false" : undefined}
    ...${rest}
  ></textarea>`;
}

export function Select({ value, onChange, options, class: cls, ...rest }) {
  return html`<select class=${cx("input", cls)} value=${value} onChange=${(e) => onChange(e.target.value)} ...${rest}>
    ${options.map(
      (o) => html`<option value=${typeof o === "object" ? o.value : o} selected=${(typeof o === "object" ? o.value : o) === value}>
        ${typeof o === "object" ? o.label : o}
      </option>`
    )}
  </select>`;
}

export function Switch({ checked, onChange, label, disabled }) {
  return html`<label class="switch" style=${disabled ? { opacity: 0.55, pointerEvents: "none" } : null}>
    <input type="checkbox" checked=${!!checked} onChange=${(e) => onChange(e.target.checked)} disabled=${disabled} />
    <span class="track"><span class="knob"></span></span>
    ${label && html`<span class="switch-label">${label}</span>`}
  </label>`;
}

export function Check({ checked, partial, onChange, label }) {
  return html`<span
    class=${cx("check", { on: checked, partial: partial && !checked })}
    role="checkbox"
    tabindex="0"
    aria-checked=${partial ? "mixed" : checked ? "true" : "false"}
    aria-label=${label}
    onClick=${(e) => {
      e.stopPropagation();
      onChange(!checked);
    }}
    onKeyDown=${(e) => {
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        onChange(!checked);
      }
    }}
  >
    <${Icon} name=${partial && !checked ? "plus" : "check"} size=${12} />
  </span>`;
}

// a segmented control with a sliding thumb
export function Segmented({ value, onChange, options }) {
  const ref = useRef();
  const [thumb, setThumb] = useState(null);
  useLayoutEffect(() => {
    const el = ref.current && ref.current.querySelector("button.on");
    if (el) setThumb({ left: el.offsetLeft, width: el.offsetWidth });
  }, [value, options.length]);
  return html`<div class="seg" ref=${ref}>
    ${thumb && html`<span class="seg-thumb" style=${{ left: thumb.left + "px", width: thumb.width + "px" }}></span>`}
    ${options.map(
      (o) => html`<button type="button" class=${cx({ on: o.value === value })} onClick=${() => onChange(o.value)}>${o.label}</button>`
    )}
  </div>`;
}

// tabs with a sliding underline
export function Tabs({ tabs, active, onChange }) {
  const ref = useRef();
  const [ink, setInk] = useState(null);
  useLayoutEffect(() => {
    const el = ref.current && ref.current.querySelector(".on");
    if (el) setInk({ left: el.offsetLeft + 8, width: el.offsetWidth - 16 });
  }, [active, tabs.map((t) => t.id).join()]);
  return html`<div class="tabs" ref=${ref} role="tablist">
    ${tabs.map(
      (t) => html`<button
        role="tab"
        aria-selected=${t.id === active}
        class=${cx({ on: t.id === active })}
        onClick=${() => onChange(t.id)}
      >
        ${t.icon && html`<${Icon} name=${t.icon} size=${16} />`} ${t.label}
        ${t.badge !== undefined && html`<span class="badge">${t.badge}</span>`}
      </button>`
    )}
    ${ink && html`<span class="tab-ink" style=${{ left: ink.left + "px", width: ink.width + "px" }}></span>`}
  </div>`;
}

export function Skeleton({ width = "100%", height = 12, style }) {
  return html`<div class="skeleton" style=${{ width, height: height + "px", ...(style || {}) }}></div>`;
}

export function Empty({ icon = "sparkle", title, children, action }) {
  return html`<div class="empty">
    <div class="empty-art"><${Icon} name=${icon} size=${28} /></div>
    <h3>${title}</h3>
    ${children && html`<p class="small" style="max-width:380px">${children}</p>`}
    ${action && html`<div style="margin-top:6px">${action}</div>`}
  </div>`;
}

export function Callout({ kind = "info", icon, children }) {
  const icons = { info: "info", warn: "warning", danger: "warning", ok: "check" };
  return html`<div class=${cx("callout", kind)}><${Icon} name=${icon || icons[kind]} size=${17} /><div>${children}</div></div>`;
}

export function CopyButton({ text, label = "Copy", size = "sm" }) {
  const [done, setDone] = useState(false);
  return html`<${IconButton}
    size=${size}
    icon=${done ? "check" : "copy"}
    label=${done ? "Copied" : label}
    class=${done ? "copy-ok" : ""}
    onClick=${(e) => {
      e.stopPropagation();
      navigator.clipboard && navigator.clipboard.writeText(text);
      setDone(true);
      setTimeout(() => setDone(false), 1400);
    }}
  />`;
}

// ------------------------------------------------------------------ overlays

// a centered sheet; closes with an exit animation
// renders children into a container on <body>, so ancestors' transforms and overflow
// can't clip overlays
export function Portal({ children }) {
  const host = useRef(null);
  if (!host.current) {
    host.current = document.createElement("div");
    host.current.className = "portal";
  }
  useLayoutEffect(() => {
    document.body.appendChild(host.current);
    return () => {
      render(null, host.current);
      host.current.remove();
    };
  }, []);
  useLayoutEffect(() => {
    render(children, host.current);
  });
  return null;
}

// focuses the first [autofocus] element (the attribute only works on page load)
export function useAutofocus(ref) {
  useEffect(() => {
    const el = ref.current && ref.current.querySelector("[autofocus]");
    if (el) setTimeout(() => el.focus(), 30);
  }, []);
}

export function Modal({ onClose, size = "", children, dismissable = true, labelledBy }) {
  const [closing, setClosing] = useState(false);
  const sheetRef = useRef();
  useAutofocus(sheetRef);
  const close = () => {
    if (closing) return;
    setClosing(true);
    setTimeout(onClose, 180);
  };
  useEffect(() => {
    const fn = (e) => {
      if (e.key === "Escape" && dismissable) {
        e.stopPropagation();
        close();
      }
    };
    window.addEventListener("keydown", fn);
    document.body.style.overflow = "hidden";
    return () => {
      window.removeEventListener("keydown", fn);
      document.body.style.overflow = "";
    };
  }, [dismissable]);
  return html`<${Portal}>
    <div
      class=${cx("backdrop", { closing })}
      onMouseDown=${(e) => dismissable && e.target === e.currentTarget && close()}
    >
      <div class=${cx("sheet", size)} ref=${sheetRef} role="dialog" aria-modal="true" aria-labelledby=${labelledBy}>
        ${typeof children === "function" ? children(close) : children}
      </div>
    </div>
  </${Portal}>`;
}

export function SheetHead({ title, subtitle, icon, onClose, children }) {
  return html`<div class="sheet-head">
    ${icon && html`<div class="field-type-badge"><${Icon} name=${icon} /></div>`}
    <div class="grow">
      <h2 id="sheet-title">${title}</h2>
      ${subtitle && html`<div class="small muted">${subtitle}</div>`}
    </div>
    ${children}
    ${onClose && html`<${IconButton} icon="close" label="Close" onClick=${onClose} />`}
  </div>`;
}

export function Menu({ trigger, items, align = "right" }) {
  const [open, setOpen] = useState(false);
  const ref = useRef();
  useClickOutside(ref, () => setOpen(false), open);
  return html`<span class="menu-anchor" ref=${ref}>
    ${trigger(() => setOpen(!open), open)}
    ${open &&
    html`<div class="menu" style=${{ top: "calc(100% + 6px)", [align]: 0 }}>
      ${items.filter(Boolean).map((item) =>
        item === "-"
          ? html`<hr />`
          : html`<button
              class=${item.danger ? "danger" : ""}
              onClick=${(e) => {
                e.stopPropagation();
                setOpen(false);
                item.onClick();
              }}
            >
              ${item.icon && html`<${Icon} name=${item.icon} size=${16} />`} ${item.label}
            </button>`
      )}
    </div>`}
  </span>`;
}

// ------------------------------------------------------------------ toasts

const toastStore = createStore([]);
let toastId = 0;

export function toast(message, kind = "ok", action) {
  const id = ++toastId;
  toastStore.set((list) => [...list, { id, message, kind, action }]);
  setTimeout(() => dismissToast(id), kind === "error" ? 6000 : 3600);
}
toast.error = (err) => toast(typeof err === "string" ? err : err && err.message ? err.message : "Something went wrong.", "error");

function dismissToast(id) {
  toastStore.set((list) => list.map((t) => (t.id === id ? { ...t, leaving: true } : t)));
  setTimeout(() => toastStore.set((list) => list.filter((t) => t.id !== id)), 200);
}

export function Toasts() {
  const list = useStore(toastStore);
  const icons = { ok: "check", error: "close", info: "info" };
  return html`<div class="toasts" aria-live="polite">
    ${list.map(
      (t) => html`<div key=${t.id} class=${cx("toast", t.kind, { leaving: t.leaving })}>
        <span class="toast-icon"><${Icon} name=${icons[t.kind] || "info"} size=${13} /></span>
        <span>${t.message}</span>
        ${t.action &&
        html`<button
          onClick=${() => {
            t.action.onClick();
            dismissToast(t.id);
          }}
        >
          ${t.action.label}
        </button>`}
      </div>`
    )}
  </div>`;
}

// ------------------------------------------------------------------ confirm

// resolves true when confirmed; `typed` asks to type a word first
export function confirmDialog({ title, message, confirm = "Confirm", danger = false, typed }) {
  return new Promise((resolve) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const done = (value) => {
      resolve(value);
      setTimeout(() => {
        render(null, host);
        host.remove();
      }, 0);
    };
    function Confirm() {
      const [text, setText] = useState("");
      const [answer, setAnswer] = useState(null);
      const ok = !typed || text === typed;
      return html`<${Modal} size="narrow" onClose=${() => done(answer === true)}>
        ${(close) => html`
          <div class="sheet-body stack">
            <div class="row top">
              <div class="field-type-badge" style=${danger ? "color:var(--danger)" : ""}>
                <${Icon} name=${danger ? "warning" : "info"} />
              </div>
              <div class="stack tight grow">
                <h3>${title}</h3>
                ${message && html`<p class="muted">${message}</p>`}
              </div>
            </div>
            ${typed &&
            html`<${Field} label=${html`Type <code>${typed}</code> to confirm`}>
              <${Input} value=${text} onInput=${setText} autofocus />
            </${Field}>`}
          </div>
          <div class="sheet-foot">
            <span class="spacer"></span>
            <${Button} variant="ghost" onClick=${() => {
              setAnswer(false);
              close();
            }}>Cancel</${Button}>
            <${Button}
              variant=${danger ? "danger" : "primary"}
              disabled=${!ok}
              autofocus=${!typed}
              onClick=${() => {
                setAnswer(true);
                done(true);
                close();
              }}
            >${confirm}</${Button}>
          </div>
        `}
      </${Modal}>`;
    }
    render(html`<${Confirm} />`, host);
  });
}

// ------------------------------------------------------------------ collection glyphs

// one of the eight muted tones, picked by a hash of the text, so a name keeps its color
export function tone(text) {
  let h = 0;
  for (const ch of String(text)) h = (h * 31 + ch.codePointAt(0)) >>> 0;
  return `var(--tone-${(h % 8) + 1})`;
}

export function CollectionGlyph({ collection, size }) {
  const name = collection.name.replace(/^_/, "");
  const style = { "--c": collection.system ? "var(--ink-3)" : tone(name), ...(size ? { width: size + "px", height: size + "px" } : {}) };
  const icon = collection.type === "auth" ? "users" : collection.type === "view" ? "view" : null;
  return html`<span class="coll-glyph" style=${style}>
    ${icon ? html`<${Icon} name=${icon} size=${size ? Math.round(size / 2.1) : 13} />` : name.slice(0, 1)}
  </span>`;
}

// animated count-up for numbers
export function CountUp({ value }) {
  const [shown, setShown] = useState(0);
  useEffect(() => {
    if (typeof value !== "number") return;
    const start = performance.now();
    const from = 0;
    let frame;
    const step = (t) => {
      const p = Math.min(1, (t - start) / 700);
      const eased = 1 - Math.pow(1 - p, 3);
      setShown(Math.round(from + (value - from) * eased));
      if (p < 1) frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [value]);
  return html`<span>${typeof value === "number" ? shown.toLocaleString() : "—"}</span>`;
}
