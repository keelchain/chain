/** Theme follows the system by default; an explicit choice is persisted. */
export type ThemeChoice = 'system' | 'light' | 'dark';

export const THEME_STORAGE_KEY = 'keel-explorer-theme';

export function readTheme(): ThemeChoice {
  try {
    const v = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (v === 'light' || v === 'dark') return v;
  } catch {
    // storage blocked
  }
  return 'system';
}

export function applyTheme(choice: ThemeChoice): void {
  const root = document.documentElement;
  if (choice === 'system') delete root.dataset['theme'];
  else root.dataset['theme'] = choice;
  try {
    if (choice === 'system') window.localStorage.removeItem(THEME_STORAGE_KEY);
    else window.localStorage.setItem(THEME_STORAGE_KEY, choice);
  } catch {
    // best effort
  }
}

export function nextTheme(c: ThemeChoice): ThemeChoice {
  return c === 'system' ? 'dark' : c === 'dark' ? 'light' : 'system';
}
