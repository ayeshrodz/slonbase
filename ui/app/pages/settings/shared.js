// What the settings sections share: their card, and the draft of the settings they edit.
import { html, useState, useEffect, useMemo } from "../../lib/core.js";
import { api, BASE } from "../../lib/api.js";
import { app } from "../../lib/state.js";
import { Icon } from "../../lib/icons.js";
import { toast } from "../../lib/ui.js";
import { applyBranding, brandingFromSettings } from "../../lib/branding.js";

export function clone(v) {
  return JSON.parse(JSON.stringify(v));
}

export function Card({ title, subtitle, icon, children, actions, class: cls = "" }) {
  return html`<div class=${"card enter " + cls}>
    <div class="card-head">
      ${icon && html`<span class="field-type-badge"><${Icon} name=${icon} size=${16} /></span>`}
      <div class="grow"><h3>${title}</h3>${subtitle && html`<div class="small muted">${subtitle}</div>`}</div>
      ${actions}
    </div>
    ${[].concat(children).some(Boolean) && html`<div class="card-body stack">${children}</div>`}
  </div>`;
}

// the settings: one draft the sections edit, saved together from the save bar
export function useSettings() {
  const [saved, setSaved] = useState(null);
  const [draft, setDraft] = useState(null);
  const [errors, setErrors] = useState({});
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    api
      .get("/api/settings")
      .then((s) => {
        setSaved(s);
        setDraft(clone(s));
      })
      .catch(toast.error);
  }, []);
  const dirty = useMemo(() => saved && JSON.stringify(saved) !== JSON.stringify(draft), [saved, draft]);
  const applySaved = (res) => {
    setSaved(res);
    setDraft(clone(res));
    applyBranding(brandingFromSettings(res, BASE));
    app.set({ hideControls: !!res.meta.hideControls });
  };
  // saves the draft, or `patch` (some sections) on its own
  const save = async (patch) => {
    setSaving(true);
    setErrors({});
    try {
      const res = await api.patch("/api/settings", patch || draft);
      if (patch) {
        // the draft already has the patched values; its other changes stay unsaved
        setSaved(res);
        applyBranding(brandingFromSettings(res, BASE));
      } else {
        applySaved(res);
      }
      toast("Settings saved.");
      return true;
    } catch (err) {
      setErrors(err.fieldErrors ? err.fieldErrors() : {});
      toast.error(err);
      return false;
    } finally {
      setSaving(false);
    }
  };
  const set = (path, value) => {
    setDraft((d) => {
      const next = clone(d);
      const keys = path.split(".");
      let obj = next;
      for (const k of keys.slice(0, -1)) obj = obj[k];
      obj[keys[keys.length - 1]] = value;
      return next;
    });
  };
  // which sections of the draft differ from the saved settings
  const changed = (section) => saved && draft && JSON.stringify(saved[section]) !== JSON.stringify(draft[section]);
  return {
    saved,
    draft,
    set,
    setDraft,
    dirty,
    changed,
    save,
    saving,
    errors,
    reset: () => {
      setDraft(clone(saved));
      applyBranding(brandingFromSettings(saved, BASE));
    },
  };
}
