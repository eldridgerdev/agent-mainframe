// Injected only by the development driver; never bundled with the application.
(() => {
  let sequence = 0;
  // Set by the driver when it re-injects this file after a page reload, so a
  // reference from before the reload can never name a new element.
  const epoch = window.__amfGuiDriverEpoch ? `${window.__amfGuiDriverEpoch}-` : '';
  const refs = new Map();
  const ids = new WeakMap();
  const controls = 'button,input,textarea,select,a[href],[role="button"],[role="tab"],[role="menuitem"],[role="switch"],[role="checkbox"]';
  const normalize = value => (value || '').replace(/\s+/g, ' ').trim();
  const visible = element => {
    if (!element.isConnected || element.closest('[hidden],[inert],[aria-hidden="true"],.xterm')) return false;
    for (let current = element; current; current = current.parentElement) {
      const style = getComputedStyle(current);
      if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse') return false;
    }
    return true;
  };
  const labelText = label => {
    const copy = label.cloneNode(true);
    for (const control of copy.querySelectorAll('input,textarea,select,button')) control.remove();
    return copy.textContent;
  };
  const name = element => normalize(
    element.getAttribute('aria-label') ||
    (element.getAttribute('aria-labelledby') || '').split(/\s+/).map(id => document.getElementById(id)?.textContent || '').join(' ') ||
    Array.from(element.labels || []).map(labelText).join(' ') ||
    (element.matches('input,textarea,select') ? element.getAttribute('placeholder') : element.textContent) ||
    element.getAttribute('title')
  );
  const ref = element => {
    if (!ids.has(element)) ids.set(element, `e${epoch}${++sequence}`);
    const id = ids.get(element);
    refs.set(id, element);
    return id;
  };
  const root = selector => {
    if (!selector) return document.body;
    const matches = document.querySelectorAll(selector);
    if (matches.length !== 1) throw new Error(`Scope matched ${matches.length} elements; expected one`);
    return matches[0];
  };
  const candidates = scope => [ ...(scope.matches(controls) ? [scope] : []), ...scope.querySelectorAll(controls) ];
  const resolve = action => {
    let matches;
    const scope = root(action.scope);
    if (action.ref) {
      const element = refs.get(action.ref);
      if (!element?.isConnected) throw new Error('Stale reference; take another snapshot');
      matches = scope.contains(element) ? [element] : [];
    } else if (action.selector) {
      matches = Array.from(scope.querySelectorAll(action.selector));
    } else if (typeof action.name === 'string') {
      matches = candidates(scope).filter(element => name(element) === action.name);
    } else {
      throw new Error('Supply ref, name, or selector');
    }
    matches = matches.filter(visible);
    if (matches.length !== 1) throw new Error(`Target matched ${matches.length} visible elements; expected one`);
    const element = matches[0];
    if (element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true') throw new Error('Target is disabled');
    return element;
  };
  const textNodes = scope => {
    const walker = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT);
    const parts = [];
    while (walker.nextNode()) {
      const parent = walker.currentNode.parentElement;
      if (parent && !parent.closest('script,style,template') && visible(parent)) parts.push(walker.currentNode.textContent);
    }
    return parts;
  };
  const text = scope => textNodes(scope).map(normalize).filter(Boolean).join('\n');
  const hasText = (scope, wanted) => {
    // A snapshot prints one text node per line, but a phrase on screen can
    // span inline elements (`Created <b>Fixture project</b>`). Compare with
    // whitespace collapsed, both as the nodes run together and as the lines a
    // snapshot shows, so either way of reading the phrase matches.
    const nodes = textNodes(scope);
    const phrase = normalize(wanted);
    return normalize(nodes.join('')).includes(phrase) || normalize(nodes.join(' ')).includes(phrase);
  };
  window.__amfGuiDriver = action => {
    if (action.op === 'snapshot') {
      // Prune detached elements without reusing their reference IDs.
      for (const [id, element] of refs) if (!element.isConnected) refs.delete(id);
      const scope = root(action.scope);
      const fullText = text(scope);
      const maxText = Math.max(0, Math.min(action.text_limit ?? 2400, 20000));
      const maxControls = Math.max(0, Math.min(action.control_limit ?? 40, 200));
      const elements = candidates(scope).filter(visible);
      return {
        title: document.title,
        text: fullText.slice(0, maxText),
        text_truncated: fullText.length > maxText,
        controls: elements.slice(0, maxControls).map(element => ({
          ref: ref(element),
          role: element.getAttribute('role') || element.tagName.toLowerCase(),
          name: name(element).slice(0, 160),
          ...(element.matches('input,textarea,select') && element.type !== 'password' ? { value: element.value.slice(0, 160) } : {}),
          ...(element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true' ? { disabled: true } : {}),
          ...(element.hasAttribute('aria-selected') ? { selected: element.getAttribute('aria-selected') === 'true' } : {}),
          ...(element.matches('input[type=checkbox],input[type=radio]') ? { checked: element.checked } : {}),
        })),
        controls_truncated: elements.length > maxControls,
      };
    }
    if (action.op === 'has_text') {
      // Polled by `wait`: a scope that has not rendered yet is "not yet". An
      // ambiguous or malformed one is still a mistake, and still throws.
      if (action.scope && document.querySelectorAll(action.scope).length === 0) return false;
      return hasText(root(action.scope), action.text);
    }
    const element = resolve(action);
    if (action.op === 'click') {
      element.click();
    } else if (action.op === 'fill') {
      if (!element.matches('input,textarea') || element.readOnly || element.matches('input[type=checkbox],input[type=radio],input[type=file]')) {
        throw new Error('Target must be an editable text input or textarea');
      }
      // Bypass React's own value tracker, then notify the application.
      const prototype = element.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(prototype, 'value').set.call(element, action.value);
      element.dispatchEvent(new Event('input', { bubbles: true }));
      element.dispatchEvent(new Event('change', { bubbles: true }));
    } else if (action.op === 'select') {
      if (!element.matches('select') || !Array.from(element.options).some(option => option.value === action.value && !option.disabled && !option.parentElement.disabled)) {
        throw new Error('Target must be a select with an enabled matching option');
      }
      element.value = action.value;
      element.dispatchEvent(new Event('change', { bubbles: true }));
    } else {
      throw new Error(`Unknown DOM operation: ${action.op}`);
    }
    return { ok: true };
  };
})();
