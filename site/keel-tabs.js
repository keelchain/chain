/* Keelchain docs: mode tabs.
 *
 * A doc section that reads differently per mode is written as
 *
 *   <div class="tabs" data-group="client">
 *   <div class="tab" data-mode="custodial"> ...markdown... </div>
 *   <div class="tab" data-mode="noncustodial"> ... </div>
 *   <div class="tab" data-mode="user"> ... </div>
 *   </div>
 *
 * This script adds the tab bar and shows one tab per block. The choice is a
 * cookie per group (path /, one year), so picking a mode in one section
 * sets it for every section of every doc page until the reader changes it.
 * Without JavaScript every tab is shown, one after the other.
 */
(() => {
  const GROUPS = {
    client: {
      cookie: 'keel_mode_client',
      fallback: 'custodial',
      modes: [
        ['custodial', 'Custodial client'],
        ['noncustodial', 'Non-custodial client'],
        ['user', 'Wallet user'],
      ],
    },
    wallet: {
      cookie: 'keel_mode_wallet',
      fallback: 'keel',
      modes: [
        ['keel', 'In Keel'],
        ['wallet', 'In your wallet'],
      ],
    },
  };

  const readCookie = (name) => {
    const m = document.cookie.match(new RegExp('(?:^|; )' + name + '=([^;]*)'));
    return m ? decodeURIComponent(m[1]) : null;
  };
  const writeCookie = (name, value) => {
    document.cookie = `${name}=${encodeURIComponent(value)}; path=/; max-age=31536000; SameSite=Lax`;
  };
  const known = (group, mode) => GROUPS[group].modes.some(([m]) => m === mode);

  // The mode in force per group: the cookie when it names a mode we know, else the fallback.
  const current = {};
  for (const g of Object.keys(GROUPS)) {
    const saved = readCookie(GROUPS[g].cookie);
    current[g] = saved && known(g, saved) ? saved : GROUPS[g].fallback;
  }

  const tabsOf = (box) => Array.from(box.children).filter((el) => el.classList.contains('tab'));

  function apply(group) {
    for (const box of document.querySelectorAll(`.tabs[data-group="${group}"]`)) {
      const tabs = tabsOf(box);
      if (!tabs.length) continue;
      // A block that lacks the chosen mode shows its first tab, without touching the choice.
      const shown = tabs.find((t) => t.dataset.mode === current[group]) || tabs[0];
      for (const t of tabs) {
        t.hidden = t !== shown;
        t.setAttribute('role', 'tabpanel');
      }
      for (const b of box.querySelectorAll(':scope > .tabbar > button')) {
        const on = b.dataset.mode === shown.dataset.mode;
        b.setAttribute('aria-selected', on ? 'true' : 'false');
        b.tabIndex = on ? 0 : -1;
      }
    }
  }

  function select(group, mode, remember = true) {
    if (!known(group, mode)) return;
    current[group] = mode;
    if (remember) writeCookie(GROUPS[group].cookie, mode);
    apply(group);
  }

  function build(box) {
    const group = box.dataset.group;
    const spec = GROUPS[group];
    if (!spec || box.querySelector(':scope > .tabbar')) return;
    const present = new Set(tabsOf(box).map((t) => t.dataset.mode));
    const bar = document.createElement('div');
    bar.className = 'tabbar';
    bar.setAttribute('role', 'tablist');
    for (const [mode, label] of spec.modes) {
      if (!present.has(mode)) continue;
      const b = document.createElement('button');
      b.type = 'button';
      b.setAttribute('role', 'tab');
      b.dataset.mode = mode;
      b.textContent = label;
      b.addEventListener('click', () => select(group, mode));
      bar.appendChild(b);
    }
    bar.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
      const buttons = Array.from(bar.querySelectorAll('button'));
      const i = buttons.findIndex((b) => b.getAttribute('aria-selected') === 'true');
      const next = buttons[(i + (e.key === 'ArrowRight' ? 1 : buttons.length - 1)) % buttons.length];
      if (next) { next.focus(); select(group, next.dataset.mode); }
      e.preventDefault();
    });
    box.prepend(bar);
  }

  // A link into a hidden tab (a heading anchor, a footnote) reveals that tab for this view.
  function revealHash() {
    if (!location.hash) return;
    let el;
    try { el = document.querySelector(decodeURIComponent(location.hash)); } catch { return; }
    if (!el) return;
    for (let tab = el.closest('.tab'); tab; tab = tab.parentElement && tab.parentElement.closest('.tab')) {
      const box = tab.parentElement;
      if (box && box.classList.contains('tabs') && GROUPS[box.dataset.group]) {
        select(box.dataset.group, tab.dataset.mode, false);
      }
    }
    el.scrollIntoView();
  }

  for (const box of document.querySelectorAll('.tabs')) build(box);
  for (const g of Object.keys(GROUPS)) apply(g);
  revealHash();
  window.addEventListener('hashchange', revealHash);
})();
