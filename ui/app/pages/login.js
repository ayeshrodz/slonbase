// Superuser sign-in (password, then OTP when MFA is on) and password reset.
import { html, useState, useRef, navigate } from "../lib/core.js";
import { api, session, ApiError } from "../lib/api.js";
import { Logo, Icon } from "../lib/icons.js";
import { Button, Field, Input, Callout, toast, useAutofocus } from "../lib/ui.js";

const SU = "/api/collections/_superusers";

export function AuthFrame({ children, title, subtitle }) {
  const ref = useRef();
  useAutofocus(ref);
  return html`<div class="auth-page" ref=${ref}>
    <div class="auth-form">
      <div class="auth-brand"><${Logo} size=${28} /><span>slonbase</span></div>
      <div class="stack loose">
        <div class="stack tight">
          <h1>${title}</h1>
          ${subtitle && html`<p class="muted">${subtitle}</p>`}
        </div>
        ${children}
      </div>
      <div class="auth-foot row"><${Icon} name="shield" size=${14} /> Superuser access to this slonbase installation.</div>
    </div>
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
    title=${mfa ? "Check your inbox" : "Welcome back"}
    subtitle=${mfa ? `Enter the one-time password sent to ${email}.` : "Sign in with your superuser account."}
  >
    <form class=${"stack" + (shake ? " shake" : "")} key=${shake} onSubmit=${submit}>
      ${error && html`<${Callout} kind="danger">${error}</${Callout}>`}
      ${mfa
        ? html`<${Field} label="One-time password">
            <${Input} value=${otp} onInput=${setOtp} autofocus inputmode="numeric" autocomplete="one-time-code" class="mono" style="letter-spacing:.3em;font-size:18px;text-align:center" />
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
