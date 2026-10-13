// Pages behind the links in auth emails (any auth collection): verification, password
// reset and email change.
import { html, useState, useEffect } from "../lib/core.js";
import { api } from "../lib/api.js";
import { Icon } from "../lib/icons.js";
import { Button, Field, Input, Callout, Spinner } from "../lib/ui.js";
import { AuthFrame } from "./login.js";

function claims(token) {
  try {
    return JSON.parse(atob(token.split(".")[1].replace(/-/g, "+").replace(/_/g, "/")));
  } catch (_) {
    return {};
  }
}

export function PublicAuth({ action, token }) {
  const collection = claims(token).collectionId || "";
  const base = `/api/collections/${encodeURIComponent(collection)}`;
  if (action === "confirm-verification") return html`<${Verify} base=${base} token=${token} />`;
  if (action === "confirm-password-reset") return html`<${Reset} base=${base} token=${token} />`;
  if (action === "confirm-email-change") return html`<${EmailChange} base=${base} token=${token} />`;
  return html`<${AuthFrame} title="Unknown link"><${Callout} kind="warn">This link isn't valid.</${Callout}></${AuthFrame}>`;
}

function Done({ message }) {
  return html`<div class="stack" style="align-items:flex-start">
    <div class="empty-art" style="width:56px;height:56px;border-radius:18px;display:grid;place-items:center;background:var(--ok-soft);color:var(--ok);animation:pop .4s var(--ease-spring)">
      <${Icon} name="check" size=${28} />
    </div>
    <p>${message}</p>
    <p class="muted small">You can close this page.</p>
  </div>`;
}

function Verify({ base, token }) {
  const [state, setState] = useState("loading");
  const [error, setError] = useState("");
  useEffect(() => {
    api
      .post(`${base}/confirm-verification`, { token }, { noAuth: true })
      .then(() => setState("done"))
      .catch((err) => {
        setError(err.message);
        setState("error");
      });
  }, []);
  return html`<${AuthFrame} title="Email verification">
    ${state === "loading" && html`<div class="row muted"><${Spinner} /> Verifying…</div>`}
    ${state === "done" && html`<${Done} message="Your email address is verified." />`}
    ${state === "error" && html`<${Callout} kind="danger">${error || "The link is invalid or has expired."}</${Callout}>`}
  </${AuthFrame}>`;
}

function Reset({ base, token }) {
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState("");
  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.post(`${base}/confirm-password-reset`, { token, password, passwordConfirm: confirm }, { noAuth: true });
      setDone(true);
    } catch (err) {
      const fields = err.fieldErrors ? Object.values(err.fieldErrors()) : [];
      setError(fields[0] || err.message);
    } finally {
      setBusy(false);
    }
  };
  return html`<${AuthFrame} title="Reset your password">
    ${done
      ? html`<${Done} message="Your password was changed." />`
      : html`<form class="stack" onSubmit=${submit}>
          ${error && html`<${Callout} kind="danger">${error}</${Callout}>`}
          <${Field} label="New password"><${Input} type="password" value=${password} onInput=${setPassword} required autofocus /></${Field}>
          <${Field} label="Confirm"><${Input} type="password" value=${confirm} onInput=${setConfirm} required /></${Field}>
          <${Button} variant="primary" size="lg" class="block" loading=${busy} type="submit">Save password</${Button}>
        </form>`}
  </${AuthFrame}>`;
}

function EmailChange({ base, token }) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState("");
  const newEmail = claims(token).newEmail || "";
  const submit = async (e) => {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.post(`${base}/confirm-email-change`, { token, password }, { noAuth: true });
      setDone(true);
    } catch (err) {
      setError(err.message);
    } finally {
      setBusy(false);
    }
  };
  return html`<${AuthFrame} title="Confirm your new email" subtitle=${newEmail && `Change the account's email to ${newEmail}.`}>
    ${done
      ? html`<${Done} message="Your email address was changed." />`
      : html`<form class="stack" onSubmit=${submit}>
          ${error && html`<${Callout} kind="danger">${error}</${Callout}>`}
          <${Field} label="Your password"><${Input} type="password" value=${password} onInput=${setPassword} required autofocus /></${Field}>
          <${Button} variant="primary" size="lg" class="block" loading=${busy} type="submit">Confirm</${Button}>
        </form>`}
  </${AuthFrame}>`;
}
