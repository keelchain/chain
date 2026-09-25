import { useEffect, useState, type FormEvent } from 'react';
import { call, type UiState } from '../ui';

type Step = 'welcome' | 'create' | 'import';

export function Onboarding({ onDone }: { onDone: (s: UiState) => void }) {
  const [step, setStep] = useState<Step>('welcome');
  if (step === 'welcome') {
    return (
      <div className="screen center">
        <h1>Keel Wallet</h1>
        <p className="muted">Your keys stay in this extension. Nothing is ever sent to a server.</p>
        <button className="primary" onClick={() => setStep('create')}>Create a new wallet</button>
        <button onClick={() => setStep('import')}>Import a seed phrase</button>
      </div>
    );
  }
  return <SetUp mode={step} onBack={() => setStep('welcome')} onDone={onDone} />;
}

function SetUp({ mode, onBack, onDone }: { mode: 'create' | 'import'; onBack: () => void; onDone: (s: UiState) => void }) {
  const [mnemonic, setMnemonic] = useState('');
  const [saved, setSaved] = useState(false);
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (mode === 'create') call<{ mnemonic: string }>('generateMnemonic').then((r) => setMnemonic(r.mnemonic), (e: unknown) => setError(String(e)));
  }, [mode]);

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    setError(null);
    if (password !== confirm) return setError('Passwords do not match.');
    if (mode === 'create' && !saved) return setError('Confirm that you wrote the phrase down.');
    setBusy(true);
    try {
      onDone(await call<UiState>('createVault', { mnemonic, password }));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const words = mnemonic.trim().split(/\s+/).filter(Boolean);
  return (
    <form className="screen" onSubmit={submit}>
      <button type="button" className="ghost back" onClick={onBack}>← Back</button>
      <h2>{mode === 'create' ? 'Your seed phrase' : 'Import seed phrase'}</h2>
      {mode === 'create' ? (
        <>
          <p className="muted">Write these 24 words down in order and keep them offline. Anyone with them controls your funds.</p>
          <ol className="words">{words.map((w, i) => <li key={i}>{w}</li>)}</ol>
          <label className="check"><input type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} /> I wrote the phrase down</label>
        </>
      ) : (
        <textarea rows={4} placeholder="24 words separated by spaces" value={mnemonic} onChange={(e) => setMnemonic(e.target.value)} autoFocus />
      )}
      <label>Password (at least 8 characters)<input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" /></label>
      <label>Confirm password<input type="password" value={confirm} onChange={(e) => setConfirm(e.target.value)} autoComplete="new-password" /></label>
      {error && <p className="error">{error}</p>}
      <button className="primary" type="submit" disabled={busy || !mnemonic}>{busy ? 'Encrypting…' : mode === 'create' ? 'Create wallet' : 'Import wallet'}</button>
    </form>
  );
}
