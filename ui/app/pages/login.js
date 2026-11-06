// Superuser sign-in (password, then OTP when MFA is on) and password reset.
import { html, useState, useRef, useEffect, navigate, useStore, cx } from "../lib/core.js";
import { api, session, ApiError, PROJECT, BASE } from "../lib/api.js";
import { Logo, Icon } from "../lib/icons.js";
import { app } from "../lib/state.js";
import { branding, logoFor } from "../lib/branding.js";
import { Button, Field, Input, Callout, toast, useAutofocus } from "../lib/ui.js";

const SU = "/api/collections/_superusers";

// the sign-in page's code panel: the PocketBase SDK, unchanged, against slonbase (one entry a line,
// since templates fold newlines in markup)
const kw = (t) => html`<span class="c-kw">${t}</span>`;
const str = (t) => html`<span class="c-str">${t}</span>`;
const fn = (t) => html`<span class="c-fn">${t}</span>`;
const CODE = [
  html`<span class="c-cm">// the PocketBase SDK, unchanged</span>`,
  html`${kw("import")} PocketBase ${kw("from")} ${str('"pocketbase"')};`,
  "",
  html`${kw("const")} pb = ${kw("new")} ${fn("PocketBase")}(${str('"https://api.example.com"')});`,
  "",
  html`${kw("const")} tasks = ${kw("await")} pb.${fn("collection")}(${str('"tasks"')})`,
  html`  .${fn("getList")}(1, 30, { filter: ${str('"done = false"')} });`,
  "",
  html`pb.${fn("collection")}(${str('"tasks"')}).${fn("subscribe")}(${str('"*"')}, (e) => {`,
  html`  console.${fn("log")}(e.action, e.record.id);`,
  "});",
];

// The sign-in page's frame, laid out as Settings → Login screen says: the form beside the code
// panel (split), in a card (centered) or alone (minimal). `primary` pages (signing in) take
// the configured title; `preview` (the settings' live preview) gives the branding to show.
export function AuthFrame({ children, title, subtitle, primary = false, preview = null }) {
  const ref = useRef();
  useAutofocus(preview ? { current: null } : ref);
  const live = useStore(branding);
  useStore(app, (s) => s.theme);
  const b = preview || live;
  const l = b.login;
  const dark = document.documentElement.dataset.theme === "dark";
  const logo = l.logo || logoFor(b, dark);
  const layout = ["split", "centered", "minimal"].includes(l.layout) ? l.layout : "split";
  const heading = (primary && l.title) || title;
  const sub = primary && l.subtitle ? l.subtitle : subtitle;
  const custom = b.appName && b.appName !== "slonbase";
  const bg = layout !== "split" && l.background === "asset" && l.backgroundImage ? { backgroundImage: `url("${l.backgroundImage}")` } : null;
  return html`<div class=${cx("auth-page", "layout-" + layout, { "bg-grid": layout !== "split" && l.background === "grid", "bg-image": !!bg, preview: !!preview })} style=${bg} ref=${ref}>
    ${layout === "split" &&
    html`<aside class="auth-art" aria-hidden="true">
      <div class="auth-art-brand"><${Logo} size=${30} /><span class="brand-name">slon<b>base</b></span></div>
      <div class="auth-art-copy">
        <h2>Your whole backend, built on PostgreSQL.</h2>
        <div class="code-window">
          <div class="code-window-bar"><i></i><i></i><i></i><span>app.js</span></div>
          <pre>${CODE.map((line, i) => html`${i ? "\n" : ""}${line}`)}</pre>
        </div>
      </div>
      <div class="auth-art-foot mono"><span class="c-prompt">$</span> slonbase serve · PostgreSQL 14+ · one binary</div>
    </aside>`}
    <div class="auth-form">
      <div class="auth-brand">
        ${logo ? html`<img class="auth-logo" src=${logo} alt=${b.appName || "Logo"} />` : html`<${Logo} size=${28} />`}
        ${!logo && html`<span class="brand-name">${custom ? html`<span class="brand-custom">${b.appName}</span>` : html`slon<b>base</b>`}</span>`}
        ${PROJECT && html`<span class="chip outline mono">${PROJECT}</span>`}
      </div>
      <div class="stack loose">
        <div class="stack tight">
          <h1>${heading}</h1>
          ${sub && html`<p class="muted">${sub}</p>`}
        </div>
        ${primary && l.notice && html`<${Callout} kind="info"><span class="pre-line">${l.notice}</span></${Callout}>`}
        ${children}
      </div>
      <div class="auth-foot">
        <div class="row"><${Icon} name="shield" size=${14} /> <span>${PROJECT ? html`Superuser access to the project <b>${PROJECT}</b>.` : custom ? html`Superuser access to <b>${b.appName}</b>.` : "Superuser access to this slonbase installation."}</span></div>
        ${(l.footerLinks || []).length > 0 &&
        html`<nav class="auth-links" aria-label="Links">${l.footerLinks.map((x) => html`<a href=${x.url} target="_blank" rel="noopener noreferrer">${x.label}</a>`)}</nav>`}
        ${l.showPoweredBy !== false && custom && html`<div class="auth-powered mono">powered by slon<b>base</b></div>`}
      </div>
    </div>
  </div>`;
}

// Signing in with an OAuth2 provider, as the PocketBase SDK does: the provider in a popup,
// which redirects to /api/oauth2-redirect; the server hands the code over realtime ("@oauth2",
// to this page's connection, named in `state`), and the code is traded for a session.
function oauth2SignIn(provider) {
  return new Promise((resolve, reject) => {
    const redirectURL = location.origin + BASE + "/api/oauth2-redirect";
    const popup = window.open("", "slonbase-oauth2", "width=520,height=680");
    if (!popup) return reject(new Error("Allow pop-ups to sign in with " + provider.displayName + "."));
    const source = new EventSource(BASE + "/api/realtime");
    let done = false;
    const finish = (fn, value) => {
      if (done) return;
      done = true;
      source.close();
      clearInterval(watch);
      fn(value);
    };
    const watch = setInterval(() => popup.closed && finish(reject, new Error("The sign-in window was closed.")), 600);
    source.addEventListener("PB_CONNECT", async (e) => {
      const { clientId } = JSON.parse(e.data);
      try {
        await api.post("/api/realtime", { clientId, subscriptions: ["@oauth2"] }, { noAuth: true });
        const url = new URL(provider.authURL + encodeURIComponent(redirectURL));
        url.searchParams.set("state", clientId);
        popup.location.href = url.toString();
      } catch (err) {
        popup.close();
        finish(reject, err);
      }
    });
    source.addEventListener("@oauth2", async (e) => {
      try {
        const msg = JSON.parse(e.data);
        popup.close();
        if (msg.error || !msg.code) throw new Error(msg.error || "The provider didn't sign you in.");
        const res = await api.post(
          "/api/collections/_superusers/auth-with-oauth2",
          { provider: provider.name, code: msg.code, codeVerifier: provider.codeVerifier, redirectURL },
          { noAuth: true }
        );
        finish(resolve, res);
      } catch (err) {
        finish(reject, err);
      }
    });
    source.onerror = () => finish(reject, new Error("Lost the connection to the server."));
  });
}

function OAuth2Buttons({ onError }) {
  const [providers, setProviders] = useState([]);
  const [busy, setBusy] = useState("");
  useEffect(() => {
    api
      .get("/api/collections/_superusers/auth-methods", { noAuth: true })
      .then((m) => setProviders((m.oauth2 && m.oauth2.enabled && m.oauth2.providers) || []))
      .catch(() => {});
  }, []);
  if (!providers.length) return null;
  const go = async (p) => {
    setBusy(p.name);
    try {
      const res = await oauth2SignIn(p);
      session.set(res.token, res.record);
      navigate(location.hash.replace(/^#/, "") || "/", true);
    } catch (err) {
      onError(err);
    } finally {
      setBusy("");
    }
  };
  return html`<div class="stack tight oauth2-buttons">
    <div class="or-line"><span>or</span></div>
    ${providers.map(
      (p) => html`<${Button} class="block" icon="key" loading=${busy === p.name} onClick=${() => go(p)}>Continue with ${p.displayName}</${Button}>`
    )}
    <div class="tiny muted">Only for existing superusers with the same, verified email.</div>
  </div>`;
}

export function Login() {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [otp, setOtp] = useState("");
  const [mfa, setMfa] = useState(null); // { mfaId, otpId }
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [shake, setShake] = useState(0);

  const fail = (err) => {
    setError(err.message || "Failed to sign in.");
    setShake((n) => n + 1);
  };

  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      if (mfa) {
        const res = await api.post(`${SU}/auth-with-otp`, { otpId: mfa.otpId, password: otp, mfaId: mfa.mfaId }, { noAuth: true });
        session.set(res.token, res.record);
        return;
      }
      const res = await api.post(`${SU}/auth-with-password`, { identity: email, password }, { noAuth: true });
      session.set(res.token, res.record);
      navigate(location.hash.replace(/^#/, "") || "/", true);
    } catch (err) {
      if (err instanceof ApiError && err.data && err.data.mfaId) {
        try {
          const sent = await api.post(`${SU}/request-otp`, { email }, { noAuth: true });
          setMfa({ mfaId: err.data.mfaId, otpId: sent.otpId });
          toast("We emailed you a one-time password.", "info");
        } catch (otpErr) {
          fail(otpErr);
        }
      } else {
        fail(err);
      }
    } finally {
      setBusy(false);
    }
  };

  return html`<${AuthFrame}
    primary=${!mfa}
    title=${mfa ? "Check your inbox" : "Welcome back"}
    subtitle=${mfa ? `Enter the one-time password sent to ${email}.` : "Sign in with your superuser account."}
  >
    <form class=${"stack" + (shake ? " shake" : "")} key=${shake} onSubmit=${submit}>
      ${error && html`<${Callout} kind="danger">${error}</${Callout}>`}
      ${mfa
        ? html`<${Field} label="One-time password">
            <${Input} value=${otp} onInput=${setOtp} autofocus inputmode="numeric" autocomplete="one-time-code" class="mono" style="letter-spacing:.3em;font-size:var(--text-xl);text-align:center" />
          </${Field}>`
        : html`
            <${Field} label="Email">
              <${Input} type="email" value=${email} onInput=${setEmail} autofocus required autocomplete="username" placeholder="you@example.com" />
            </${Field}>
            <${Field} label="Password" extra=${html`<a class="small" href="#/request-password-reset">Forgot?</a>`}>
              <${Input} type="password" value=${password} onInput=${setPassword} required autocomplete="current-password" />
            </${Field}>
          `}
      <${Button} variant="primary" size="lg" class="block" loading=${busy} type="submit" iconRight="arrowRight">
        ${mfa ? "Verify and sign in" : "Sign in"}
      </${Button}>
      ${mfa && html`<${Button} variant="ghost" onClick=${() => setMfa(null)}>Use another account</${Button}>`}
    </form>
    ${!mfa && html`<${OAuth2Buttons} onError=${fail} />`}
  </${AuthFrame}>`;
}

export function PasswordResetRequest() {
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    try {
      await api.post(`${SU}/request-password-reset`, { email }, { noAuth: true });
      setSent(true);
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  return html`<${AuthFrame} title="Reset your password" subtitle="We'll email you a link to choose a new one.">
    ${sent
      ? html`<${Callout} kind="ok">If <b>${email}</b> belongs to a superuser, a reset link is on its way.</${Callout}>`
      : html`<form class="stack" onSubmit=${submit}>
          <${Field} label="Email"><${Input} type="email" value=${email} onInput=${setEmail} required autofocus /></${Field}>
          <${Button} variant="primary" size="lg" class="block" loading=${busy} type="submit">Send reset link</${Button}>
        </form>`}
    <a href="#/" class="small row"><${Icon} name="chevronLeft" size=${14} /> Back to sign in</a>
  </${AuthFrame}>`;
}

export function PasswordResetConfirm({ token }) {
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [errors, setErrors] = useState({});
  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    try {
      await api.post(`${SU}/confirm-password-reset`, { token, password, passwordConfirm: confirm }, { noAuth: true });
      toast("Password changed. You can sign in now.");
      navigate("/", true);
    } catch (err) {
      setErrors(err.fieldErrors ? err.fieldErrors() : {});
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  return html`<${AuthFrame} title="Choose a new password">
    <form class="stack" onSubmit=${submit}>
      <${Field} label="New password" error=${errors.password}><${Input} type="password" value=${password} onInput=${setPassword} required autofocus /></${Field}>
      <${Field} label="Confirm" error=${errors.passwordConfirm}><${Input} type="password" value=${confirm} onInput=${setConfirm} required /></${Field}>
      <${Button} variant="primary" size="lg" class="block" loading=${busy} type="submit">Save password</${Button}>
    </form>
  </${AuthFrame}>`;
}
